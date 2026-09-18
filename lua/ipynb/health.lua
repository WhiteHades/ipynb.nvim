local M = {}

local function last_line(value)
  if not value then
    return nil
  end

  return tostring(value):gsub("%s+$", ""):match("([^\n]+)$")
end

local function report_command(label, command, required)
  if type(command) ~= "string" or command == "" then
    if required then
      vim.health.error(label .. " is not configured")
    else
      vim.health.info(label .. " is disabled")
    end
    return false
  end

  if vim.fn.executable(command) == 1 then
    vim.health.ok(label .. ": " .. command)
    return true
  end

  if required then
    vim.health.error(label .. " was not found: " .. command)
  else
    vim.health.warn(label .. " was not found: " .. command)
  end
  return false
end

local function report_module(label, module, required)
  local ok, error_message = pcall(require, module)
  if ok then
    vim.health.ok(label .. " is available")
    return true
  end

  local message = label .. " is unavailable"
  local detail = last_line(error_message)
  if detail then
    message = message .. ": " .. detail
  end
  if required then
    vim.health.error(message)
  else
    vim.health.warn(message)
  end
  return false
end

local function run_import(interpreter, module)
  if vim.system then
    local result = vim.system({ interpreter, "-c", "import " .. module }, { text = true }):wait()
    return result.code == 0, result.stderr or result.stdout
  end

  local output = vim.fn.system({ interpreter, "-c", "import " .. module })
  return vim.v.shell_error == 0, output
end

local function report_import(interpreter, label, module)
  local ok, output = run_import(interpreter, module)
  if ok then
    vim.health.ok(label .. " imports")
    return true
  end

  local message = label .. " failed to import"
  local detail = last_line(output)
  if detail then
    message = message .. ": " .. detail
  end
  vim.health.error(message)
  return false
end

local function report_parser(language)
  if not vim.treesitter or not vim.treesitter.language then
    vim.health.error("treesitter is unavailable")
    return false
  end

  local ok = pcall(vim.treesitter.language.inspect, language)
  if ok then
    vim.health.ok("treesitter parser: " .. language)
    return true
  end

  vim.health.error("treesitter parser is missing: " .. language)
  return false
end

local function report_ipynb_commands()
  local missing = {}
  for _, command in ipairs({ "IpynbInit", "IpynbEnterOutput", "IpynbHideOutput", "IpynbImportOutput", "IpynbExportOutput" }) do
    if vim.fn.exists(":" .. command) ~= 2 then
      table.insert(missing, command)
    end
  end

  if #missing == 0 then
    vim.health.ok("ipynb commands are available")
    local function registered(name)
      return vim.fn.exists("*" .. name) == 1 or vim.fn.exists("#FuncUndefined#" .. name) == 1
    end
    if not registered("IpynbNotebookRead") or not registered("IpynbNotebookWrite") then
      vim.health.warn("fast notebook I/O is not registered; run :UpdateRemotePlugins and restart Neovim")
    end
    return true
  end

  vim.health.error("ipynb commands are missing: " .. table.concat(missing, ", "))
  return false
end

function M.check()
  vim.health.start("ipynb.nvim")
  if vim.fn.has("nvim-0.12") == 0 then
    vim.health.error("neovim 0.12 or newer is required by the bundled dependency setup")
  end

  local ok, ipynb = pcall(require, "ipynb")
  if not ok then
    vim.health.error("ipynb.nvim could not be loaded: " .. tostring(ipynb))
    return
  end

  local options = ipynb.options or {}
  local python = options.python or "python3"
  local jupytext = options.jupytext or "jupytext"
  local kernel_python = options.kernel_python
  local root = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":h:h:h")
  local binary = options.binary or root .. "/native/target/release/ipynb-engine"
  local native = options.backend ~= "python" and vim.fn.executable(binary) == 1
  if native then
    vim.health.ok("Rust notebook engine: " .. binary)
  elseif options.backend == "python" then
    vim.health.info("Python compatibility backend selected")
  else
    vim.health.warn("Rust engine is not built; run :IpynbInstall. Python compatibility backend is active.")
  end

  local python_ok = report_command("python", python, true)
  report_command("jupytext", jupytext, true)
  if kernel_python then
    report_command("kernel python", kernel_python, true)
  else
    vim.health.info("kernel python follows the configured python runtime")
  end

  report_ipynb_commands()
  report_module("jupytext neovim plugin", "jupytext", true)
  report_module("quarto", "quarto", true)
  report_module("otter", "otter", true)

  for _, parser in ipairs({ "markdown", "markdown_inline", "python" }) do
    report_parser(parser)
  end

  if python_ok then
    for _, dependency in ipairs({
      { "pynvim", "pynvim" },
      { "ipykernel", "ipykernel" },
      { "jupyter client", "jupyter_client" },
      { "jupytext python package", "jupytext" },
      { "nbformat", "nbformat" },
      { "numpy", "numpy" },
      { "matplotlib", "matplotlib" },
    }) do
      if native and dependency[2] == "pynvim" then
        vim.health.info("Rust backend does not use the Python remote host")
      else
        report_import(python, dependency[1], dependency[2])
      end
    end
    if native then
      vim.health.info("Notebook JSON uses Rust serde_json")
    elseif vim.env.IPYNB_DISABLE_RUST == "1" then
      vim.health.info("Rust JSON parser is disabled; using Python")
    elseif run_import(python, "jiter") then
      vim.health.ok("Rust JSON parser (jiter) is available")
    else
      vim.health.info("Optional Rust JSON parser is not installed; using Python")
    end
  end

  if options.images and #vim.api.nvim_list_uis() > 0 then
    report_command("imagemagick", "magick", true)
    report_module("image.nvim", "image", true)
    if python_ok then
      report_import(python, "pillow", "PIL")
    end
  else
    vim.health.info("image output is disabled or this session is headless")
  end
end

return M
