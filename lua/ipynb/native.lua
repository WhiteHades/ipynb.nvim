local M = {}
local root = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":h:h:h")
local job, options, serial = nil, nil, 0
local pending, carry = {}, ""
local state = { kernels = {}, buffers = {} }
local runtime_options = {}
local cell_status, copied_output = {}, {}
local closing = false
local initialized = false

local function ui() return require("ipynb.native_ui") end
local function notify(message) vim.notify(message, vim.log.levels.ERROR, { title = "ipynb" }) end

local function user_event(pattern, data)
  local event = { pattern = pattern, modeline = false }
  if data ~= nil then event.data = data end
  vim.api.nvim_exec_autocmds("User", event)
end

local function configured_options()
  local result = {}
  local defaults = {
    numpy_legacy_repr = true, image_provider = "none", image_location = "both",
    auto_image_popup = false, auto_open_html_in_browser = false, auto_open_output = false,
    enter_output_behavior = "open_then_enter", output_crop_border = true,
    output_show_exec_time = true, output_show_more = false, output_virt_lines = false,
    output_win_border = { "", "━", "", "" }, output_win_cover_gutter = true,
    output_win_hide_on_leave = true, output_win_max_height = 999999, output_win_max_width = 999999,
    output_win_zindex = 50, output_win_style = false, use_border_highlights = false,
    virt_lines_off_by_1 = false, virt_text_max_lines = 12, virt_text_output = false,
    virt_text_truncate = "bottom", wrap_output = false, floating_window_focus = "top",
    limit_output_chars = 1000000, cover_empty_lines = false, cover_lines_starting_with = {},
    copy_output = false, show_mimetype_debug = false, split_direction = "right", split_size = 40,
    save_path = vim.fn.stdpath("data") .. "/ipynb.nvim",
  }
  for name, default in pairs(defaults) do
    local value = vim.g["ipynb_" .. name]
    result[name] = value == nil and default or value
  end
  result.open_cmd = vim.g.ipynb_open_cmd
  return result
end

local function receive(message)
  if message.id then
    local request = pending[message.id]
    if request then
      request.result, request.error = message.result, message.error
      request.comparison = message.comparison
      request.done = true
    end
  elseif message.event == "cells" then
    ui().sync(message.buf, message.cells)
    for _, cell in ipairs(message.cells or {}) do
      local key = tostring(message.buf) .. ":" .. tostring(cell.id)
      local previous = cell_status[key]
      cell_status[key] = cell.status
      if tonumber(message.buf) == vim.api.nvim_get_current_buf() and not cell.old
        and cell.status == "done" and previous ~= nil and previous ~= "done" then
        if runtime_options.auto_open_html_in_browser and cell.html and cell.html ~= vim.NIL then
          ui().open_browser(cell.id)
        end
        if runtime_options.auto_image_popup and type(cell.images) == "table" and #cell.images > 0 then
          ui().image_popup(cell.id)
        end
      end
      if runtime_options.copy_output and not cell.old and type(cell.lines) == "table" then
        local text = table.concat(cell.lines, "\n")
        if text ~= "" and copied_output[key] ~= text then
          copied_output[key] = text
          pcall(vim.fn.setreg, "+", text)
        end
      end
    end
  elseif message.event == "state" then
    state = message.state
  elseif message.event == "ready" then
    user_event("IpynbKernelReady", { kernel_id = message.kernel })
  elseif message.event == "input" then
    local function reply(value)
      if job then
        serial = serial + 1
        vim.fn.chansend(job, vim.json.encode({ id = serial, method = "stdin",
          params = { kernel = message.kernel, value = value or "" } }) .. "\n")
      end
    end
    if message.password then
      local ok, value = pcall(vim.fn.inputsecret, message.prompt or "")
      reply(ok and value or "")
    else
      vim.ui.input({ prompt = message.prompt or "Input: " }, reply)
    end
  elseif message.event == "error" then
    notify(message.message)
  end
end

local function start()
  if job then return end
  local binary = options.binary or root .. "/native/target/release/ipynb-engine"
  if vim.fn.executable(binary) ~= 1 then error("Rust engine is missing; run :IpynbInstall") end
  carry, closing = "", false
  job = vim.fn.jobstart({ binary, "--python", options.python, "--converter", root .. "/native/converter.py" }, {
    on_stdout = function(_, data)
      local chunk = table.concat(data, "\n")
      vim.schedule(function()
        carry = carry .. chunk
        while true do
          local at = carry:find("\n", 1, true)
          if not at then break end
          local line = carry:sub(1, at - 1)
          carry = carry:sub(at + 1)
          if line ~= "" then
            local ok, message = pcall(vim.json.decode, line)
            if ok then
              local handled, err = pcall(receive, message)
              if not handled then notify(tostring(err)) end
            else notify("Invalid response from the notebook engine") end
          end
        end
      end)
    end,
    on_stderr = function(_, data)
      local message = table.concat(data, "\n"):gsub("%s+$", "")
      if message ~= "" then vim.schedule(function() if not closing then notify(message) end end) end
    end,
    on_exit = function(_, code)
      job = nil
      for _, request in pairs(pending) do request.error, request.done = "Notebook engine stopped", true end
      for buf in pairs(state.buffers or {}) do ui().clear(tonumber(buf)) end
      state = { kernels = {}, buffers = {} }
      cell_status, copied_output = {}, {}
      initialized = false
      if code ~= 0 and not closing then vim.schedule(function() notify("Notebook engine exited: " .. code) end) end
    end,
  })
  if job <= 0 then job = nil; error("Could not start the Rust notebook engine") end
  runtime_options = configured_options()
  M.request("configure", runtime_options)
end

function M.request(method, params)
  start()
  params = params or {}
  for attempt = 1, 2 do
    serial = serial + 1
    local id = serial
    local request = {}
    pending[id] = request
    vim.fn.chansend(job, vim.json.encode({ id = id, method = method, params = params }) .. "\n")
    local ok = vim.wait(30000, function() return request.done == true end, 1)
    pending[id] = nil
    if not ok then error("Notebook engine timed out: " .. method) end
    if request.error then error(request.error) end
    if not request.comparison then return request.result end
    if attempt == 2 then error("Notebook source comparison did not converge") end

    local clean = require("ipynb.molten_remove_comments").remove_comments
    params.normalized = params.normalized or {}
    for language, sources in pairs(request.comparison) do
      if type(language) ~= "string" or type(sources) ~= "table" then
        error("Notebook engine requested an invalid source comparison")
      end
      language = ({ python3 = "python", ipython = "python" })[language] or language
      params.normalized[language] = params.normalized[language] or {}
      for _, source in ipairs(sources) do
        if type(source) ~= "string" then error("Notebook engine requested an invalid source comparison") end
        local parsed, normalized = pcall(clean, source .. "\n", language)
        if not parsed then error("Could not compare notebook source: " .. tostring(normalized)) end
        params.normalized[language][source] = normalized
      end
    end
  end
end

local function buffer_kernels(buf)
  return state.buffers[tostring(buf)] or state.buffers[buf] or {}
end

local function final_kernels(buf)
  local result = {}
  for _, id in ipairs(buffer_kernels(buf)) do
    local used_elsewhere = false
    for other, ids in pairs(state.buffers) do
      if tonumber(other) ~= tonumber(buf) and vim.tbl_contains(ids, id) then
        used_elsewhere = true
        break
      end
    end
    if not used_elsewhere then result[#result + 1] = id end
  end
  return result
end

local function clear_buffer_state(buf)
  local prefix = tostring(buf) .. ":"
  for key in pairs(cell_status) do if vim.startswith(key, prefix) then cell_status[key] = nil end end
  for key in pairs(copied_output) do if vim.startswith(key, prefix) then copied_output[key] = nil end end
end

local function context(lines)
  local buf = vim.api.nvim_get_current_buf()
  local cursor = vim.api.nvim_win_get_cursor(0)
  local params = { buf = buf, path = vim.api.nvim_buf_get_name(buf),
    cursor = { cursor[1] - 1, cursor[2] }, positions = ui().positions(buf) }
  if lines then params.lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false) end
  return params
end

local function current()
  local p = context()
  local cell = ui().current(p.buf, p.cursor[1], p.cursor[2])
  if not cell then error("Place the cursor inside a cell with output") end
  return cell, p
end

local function kernel(params, requested, callback)
  local kernels = buffer_kernels(params.buf)
  if requested and requested ~= "" then params.kernel = requested; return callback(params) end
  if #kernels == 1 then params.kernel = kernels[1]; return callback(params) end
  if #kernels == 0 then error("Initialize a kernel with :IpynbInit") end
  vim.ui.select(kernels, { prompt = "Select a kernel: " }, function(id)
    if id then params.kernel = id; callback(params) end
  end)
end

local function evaluate(first, last, requested, source)
  local params = context()
  params.begin, params["end"] = first, last
  local final_line = vim.api.nvim_buf_get_lines(params.buf, last[1], last[1] + 1, false)[1] or ""
  if last[2] < 0 then last[2] = #final_line + last[2] + 1 end
  last[2] = math.min(#final_line, last[2])
  params.source = source or table.concat(vim.api.nvim_buf_get_text(params.buf, first[1], first[2], last[1], last[2], {}), "\n")
  return kernel(params, requested, function(p) return M.request("execute", p) end)
end

local function saved_state_path()
  local name = vim.api.nvim_buf_get_name(0):gsub("%%", "%%%%"):gsub("/", "%%")
  return (vim.g.ipynb_save_path or vim.fn.stdpath("data") .. "/ipynb.nvim") .. "/" .. name .. ".json"
end

local function notebook_path(path)
  return path:sub(-6) == ".ipynb" and path or (path .. ".ipynb")
end

function M.call(name, args, bang)
  args = args or {}
  if name == "IpynbNotebookRead" then return M.request("read", { path = args[1], template = args[2] }) end
  if name == "IpynbNotebookWrite" then
    local p = context()
    p.path, p.text, p.mtime = args[1], args[2], args[3]
    local result = M.request("write", p)
    vim.b.ipynb_native_write = true
    return result
  end
  if name == "IpynbAvailableKernels" then return M.request("available") end
  if name == "IpynbRunningKernels" then
    return args[1] and buffer_kernels(vim.api.nvim_get_current_buf()) or state.kernels
  end
  if name == "IpynbStatusLineKernels" then return table.concat(M.call("IpynbRunningKernels", args), " ") end
  if name == "IpynbStatusLineInit" then return initialized and "Ipynb" or "" end
  if name == "IpynbInit" then
    local p = context()
    local shared = args[1] == "shared"
    local requested = args[shared and 2 or 1]
    local function init(id, share)
      p.kernel, p.shared = id, share
      if not share then user_event("IpynbInitPre") end
      local actual = M.request("init", p)
      initialized = true
      if not share then
        user_event("IpynbInitPost")
      end
      return actual
    end
    if requested then return init(requested, shared) end
    local choices = {}
    if not shared then for _, id in ipairs(M.request("available")) do choices[#choices+1] = { id=id, shared=false } end end
    for _, id in ipairs(state.kernels) do choices[#choices+1] = { id=id, shared=true } end
    vim.ui.select(choices, { prompt = "Select a kernel: ", format_item = function(k) return k.id .. (k.shared and " (shared)" or "") end },
      function(k) if k then init(k.id, k.shared) end end)
    return
  end
  if name == "IpynbDeinit" or name == "IpynbOnBufferUnload" then
    local unload = name == "IpynbOnBufferUnload"
    local requested = args[1]
    if unload and requested == nil then requested = tonumber(vim.fn.expand("<abuf>")) end
    local buf = tonumber(requested) or vim.api.nvim_get_current_buf()
    local attached = buffer_kernels(buf)
    if #attached == 0 then
      if unload then return end
      error("Ipynb is not initialized in this buffer; run `:IpynbInit` to initialize.")
    end
    local final = final_kernels(buf)
    for _ = 1, #final do user_event("IpynbDeinitPre") end
    if job then M.request("deinit", { buf = buf, positions = ui().positions(buf) }) end
    ui().clear(buf)
    clear_buffer_state(buf)
    if vim.api.nvim_buf_is_valid(buf) then vim.b[buf].notebook_kernel_initialized = false end
    for _ = 1, #final do user_event("IpynbDeinitPost") end
    return
  end
  if name == "IpynbOnExitPre" then
    closing = true
    local count = #(state.kernels or {})
    for _ = 1, count do user_event("IpynbDeinitPre") end
    if job then
      local active = job
      pcall(M.request, "shutdown")
      if active then vim.fn.jobwait({ active }, 1000) end
    end
    for _ = 1, count do user_event("IpynbDeinitPost") end
    return
  end
  if name == "IpynbTick" then if job then state = M.request("tick") end; return end
  if name == "IpynbTickInput" then return end -- Input requests arrive from the engine without polling.
  if name == "IpynbSendStdin" then return M.request("stdin", { kernel=args[1], value=args[2] }) end
  if name == "IpynbEvaluateRange" then
    local index, requested = 1, nil
    if type(args[1]) == "string" then requested, index = args[1], 2 end
    local span = #args - index + 1
    if span ~= 2 and span ~= 4 then error("IpynbEvaluateRange requires two or four coordinates") end
    for offset = 0, span - 1 do
      if tonumber(args[index + offset]) == nil then error("IpynbEvaluateRange coordinates must be numbers") end
      args[index + offset] = tonumber(args[index + offset])
    end
    return evaluate({ args[index]-1, (args[index+2] or 1)-1 }, { args[index+1]-1, (args[index+3] or 0)-1 }, requested)
  end
  if name == "IpynbEvaluateLine" then
    local row = vim.api.nvim_win_get_cursor(0)[1]-1
    return evaluate({row,0}, {row,-1}, args[1])
  end
  if name == "IpynbEvaluateArgument" then
    local requested = vim.tbl_contains(buffer_kernels(vim.api.nvim_get_current_buf()), args[1]) and table.remove(args,1) or nil
    return evaluate({0,0}, {0,0}, requested, table.concat(args," "))
  end
  if name == "IpynbEvaluateVisual" or name == "IpynbOperatorfunc" then
    local first_mark, last_mark = "'<", "'>"
    if name == "IpynbOperatorfunc" then first_mark, last_mark = "'[", "']" end
    local first, last = vim.fn.getpos(first_mark), vim.fn.getpos(last_mark)
    if first[2] == 0 or last[2] == 0 then error("No selection found") end
    local first_text, last_text = vim.fn.getline(first[2]), vim.fn.getline(last[2])
    local first_col = math.max(0, math.min(first[3], #first_text) - 1)
    local last_col = math.min(last[3], #last_text)
    if name == "IpynbOperatorfunc" then
      local kind = args[1]
      if kind == "line" then
        first_col, last_col = 0, #last_text
      elseif kind ~= "char" then
        error("this kind of selection is not supported: '" .. tostring(kind) .. "'")
      end
    end
    return evaluate({ first[2] - 1, first_col }, { last[2] - 1, last_col },
      name == "IpynbEvaluateVisual" and args[1] or nil)
  end
  if name == "IpynbEvaluateOperator" then
    vim.go.operatorfunc = "IpynbOperatorfunc"
    vim.api.nvim_feedkeys("g@", "n", false)
    return
  end
  if name == "IpynbDefineCell" then
    if #args == 0 then return end
    if #args ~= 2 and #args ~= 3 then error("IpynbDefineCell requires two or three arguments") end
    if tonumber(args[1]) == nil or tonumber(args[2]) == nil then
      error("IpynbDefineCell requires start and end lines")
    end
    args[1], args[2] = tonumber(args[1]), tonumber(args[2])
    local p = context(); p.begin, p["end"] = {args[1]-1,0}, {args[2]-1,#vim.fn.getline(args[2])}
    p.source = table.concat(vim.api.nvim_buf_get_text(p.buf,p.begin[1],0,p["end"][1],p["end"][2],{}),"\n")
    return kernel(p,args[3],function(value) return M.request("define",value) end)
  end
  if name == "IpynbReevaluateAll" or name == "IpynbReevaluateCell" then
    local p = context()
    if name == "IpynbReevaluateCell" then local cell = current(); p.cell = cell.id end
    return M.request("reevaluate",p)
  end
  if name == "IpynbInterrupt" or name == "IpynbRestart" then
    local p = context(); p.clear = bang or false
    return kernel(p,args[1],function(value) return M.request(name == "IpynbInterrupt" and "interrupt" or "restart",value) end)
  end
  if name == "IpynbDelete" then
    local p = context()
    if not bang then local cell = current(); p.cell = cell.id end
    return M.request("delete",p)
  end
  if name == "IpynbImportOutput" or name == "IpynbExportOutput" then
    local p = context(true); p.path = notebook_path(args[1] or p.path); p.overwrite = bang or false
    return kernel(p,args[2],function(value) return M.request(name == "IpynbImportOutput" and "import" or "export",value) end)
  end
  if name == "IpynbSave" then
    local p = context(true)
    p.path = args[1] or saved_state_path()
    return kernel(p, args[2], function(value) return M.request("save", value) end)
  end
  if name == "IpynbLoad" then
    local p = context(true)
    p.shared = args[1] == "shared"
    p.path = args[p.shared and 2 or 1] or saved_state_path()
    local result = M.request("load", p)
    initialized = true
    return result
  end
  if name == "IpynbUpdateOption" then
    if #args ~= 2 then error("IpynbUpdateOption requires an option name and value") end
    local key = tostring(args[1]):gsub("^ipynb_","")
    if runtime_options[key] == nil and key ~= "open_cmd" then error("Invalid Ipynb option: " .. key) end
    vim.g["ipynb_"..key] = args[2]
    runtime_options = configured_options()
    ui().configure(runtime_options)
    return M.request("options",{[key]=args[2]})
  end
  if name == "IpynbInfo" then
    local attached = buffer_kernels(vim.api.nvim_get_current_buf())
    local lines = { " press q or <esc> to close this window", "", " Ipynb Info", "",
      " Initialized: " .. tostring(initialized), "", (" %d active kernel(s), attached to current buffer:"):format(#attached) }
    for _, id in ipairs(attached) do lines[#lines + 1] = "   " .. id end
    local others = {}
    for _, id in ipairs(state.kernels) do if not vim.tbl_contains(attached, id) then others[#others + 1] = id end end
    lines[#lines + 1], lines[#lines + 2] = "", (" %d active kernel(s), not attached to this buffer:"):format(#others)
    for _, id in ipairs(others) do lines[#lines + 1] = "   " .. id end
    local available = M.request("available")
    local inactive = {}
    for _, id in ipairs(available) do if not vim.tbl_contains(state.kernels, id) then inactive[#inactive + 1] = id end end
    lines[#lines + 1], lines[#lines + 2] = "", (" %d inactive kernel(s):"):format(#inactive)
    for _, id in ipairs(inactive) do lines[#lines + 1] = "   " .. id end
    local buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
    local width = math.max(1, math.min(vim.o.columns - 2, math.floor(vim.o.columns * 0.8)))
    local height = math.max(1, math.min(vim.o.lines - 2, #lines, math.floor(vim.o.lines * 0.75)))
    vim.api.nvim_open_win(buf, true, { relative="editor", row=math.floor((vim.o.lines-height)/2),
      col=math.floor((vim.o.columns-width)/2), width=width, height=height, style="minimal" })
    local close = function() if vim.api.nvim_buf_is_valid(buf) then vim.api.nvim_buf_delete(buf, { force=true }) end end
    vim.keymap.set("n", "q", close, { buffer=buf, silent=true })
    vim.keymap.set("n", "<Esc>", close, { buffer=buf, silent=true })
    return
  end
  if name == "IpynbNext" or name == "IpynbPrev" or name == "IpynbGoto" then
    local p = context()
    local cells = ui().cells(p.buf)
    local function before(left, right)
      return left[1] < right[1] or (left[1] == right[1] and left[2] < right[2])
    end
    local function contains(cell, position)
      return not before(position, cell.begin) and before(position, cell["end"])
    end
    table.sort(cells, function(a, b) return before(a.begin, b.begin) end)
    if #cells == 0 then return end
    local count = tonumber(args[1]) or 1
    local index
    if name == "IpynbGoto" then
      index = (count - 1) % #cells + 1
    else
      if name == "IpynbPrev" then count = -count end
      if before(p.cursor, cells[1].begin) then
        index = 1
        if count > 0 then count = count - 1 end
      elseif before(cells[#cells]["end"], p.cursor) then
        index = #cells
        if count < 0 then count = count + 1 end
      else
        for candidate, cell in ipairs(cells) do
          local next_cell = cells[candidate + 1]
          if contains(cell, p.cursor) or (next_cell and before(cell["end"], p.cursor) and before(p.cursor, next_cell.begin)) then
            index = candidate
            break
          end
        end
      end
      if not index then return end
      index = (index - 1 + count) % #cells + 1
    end
    local target = cells[index].begin
    vim.api.nvim_win_set_cursor(0, { target[1] + 1, target[2] })
    return
  end
  if name == "IpynbHideOutput" then return ui().hide() end
  if name == "IpynbBufLeave" then return ui().hide() end
  if name == "IpynbOnCursorMoved" or name == "IpynbOnWinScrolled" or name == "IpynbUpdateInterface" then
    if ui().refresh then ui().refresh(vim.api.nvim_get_current_buf()) end
    return
  end
  if name == "IpynbToggleVirtual" and bang then
    local cells = ui().cells(vim.api.nvim_get_current_buf())
    return #cells > 0 and ui().toggle(cells[1].id, true) or false
  end
  local cell = current()
  if name == "IpynbEnterOutput" then return ui().open(cell.id) end
  if name == "IpynbShowOutput" then return ui().show(cell.id) end
  if name == "IpynbImagePopup" then return ui().image_popup(cell.id) end
  if name == "IpynbOpenInBrowser" then return ui().open_browser(cell.id) end
  if name == "IpynbYankOutput" then return ui().yank(cell.id,bang and "+" or '"') end
  if name == "IpynbToggleVirtual" then return ui().toggle(cell.id,bang) end
  error("Unknown notebook operation: "..name)
end

function M.register()
  local commands = {
    IpynbInit={nargs="*",complete="file"},IpynbDeinit={nargs=0},IpynbInfo={nargs=0},
    IpynbNext={nargs="*"},IpynbPrev={nargs="*"},IpynbGoto={nargs="*"},
    IpynbEnterOutput={nargs=0},IpynbOpenInBrowser={nargs=0},IpynbImagePopup={nargs=0},
    IpynbEvaluateArgument={nargs="*"},IpynbEvaluateVisual={nargs="*"},IpynbEvaluateOperator={nargs=0},
    IpynbEvaluateLine={nargs="*"},IpynbReevaluateAll={nargs=0},IpynbReevaluateCell={nargs=0},
    IpynbInterrupt={nargs="*"},IpynbRestart={nargs="*",bang=true},IpynbDelete={nargs=0,bang=true},
    IpynbShowOutput={nargs=0},IpynbHideOutput={nargs=0},IpynbImportOutput={nargs="*"},
    IpynbExportOutput={nargs="*",bang=true},IpynbSave={nargs="*"},IpynbLoad={nargs="*"},
    IpynbToggleVirtual={nargs=0,bang=true},IpynbYankOutput={nargs=0,bang=true},
  }
  for name, command_options in pairs(commands) do
    vim.api.nvim_create_user_command(name,function(args)
      local ok,err = pcall(M.call,name,args.fargs,args.bang)
      if not ok then notify(tostring(err)) end
    end,vim.tbl_extend("force", command_options, {force=true}))
  end
  for _,name in ipairs({"IpynbNotebookRead","IpynbNotebookWrite","IpynbAvailableKernels","IpynbRunningKernels",
    "IpynbStatusLineKernels","IpynbStatusLineInit","IpynbEvaluateRange","IpynbUpdateOption","IpynbBufLeave",
    "IpynbOnBufferUnload","IpynbOnExitPre","IpynbTick","IpynbTickInput","IpynbSendStdin","IpynbUpdateInterface",
    "IpynbOnCursorMoved","IpynbOnWinScrolled","IpynbOperatorfunc","IpynbDefineCell"}) do
    vim.cmd(("function! %s(...) abort\nreturn luaeval(\"require('ipynb.native').call(_A[1], _A[2])\", ['%s', a:000])\nendfunction"):format(name,name))
  end
end

function M.setup(opts)
  options = opts
  local binary = opts.binary or root .. "/native/target/release/ipynb-engine"
  if opts.backend == "python" then return false end
  if vim.fn.executable(binary) ~= 1 then
    if opts.backend == "rust" then error("Rust engine is missing; run :IpynbInstall") end
    return false
  end
  runtime_options = configured_options()
  ui().configure(runtime_options)
  M.register()
  local group = vim.api.nvim_create_augroup("IpynbNative",{clear=true})
  -- Neovim sources remote-plugin command stubs after package setup. Reclaim
  -- this plugin's public commands once that runtime file has finished.
  vim.api.nvim_create_autocmd("SourcePost",{group=group,pattern="*/plugin/rplugin.vim",callback=M.register})
  vim.api.nvim_create_autocmd("BufUnload",{group=group,callback=function(ev)
    if #buffer_kernels(ev.buf)>0 then pcall(M.call,"IpynbOnBufferUnload",{ev.buf}) end
  end})
  vim.api.nvim_create_autocmd("ExitPre",{group=group,callback=function()
    if job then M.call("IpynbOnExitPre") end
  end})
  vim.api.nvim_create_autocmd({"CursorMoved","CursorMovedI"},{group=group,callback=function()
    if job then M.call("IpynbOnCursorMoved") end
  end})
  vim.api.nvim_create_autocmd("WinScrolled",{group=group,callback=function()
    if job then M.call("IpynbOnWinScrolled") end
  end})
  vim.api.nvim_create_autocmd({"WinResized","VimResized","BufEnter"},{group=group,callback=function()
    if job then M.call("IpynbUpdateInterface") end
  end})
  vim.api.nvim_create_autocmd("BufLeave",{group=group,callback=function()
    if job then M.call("IpynbBufLeave") end
  end})
  return true
end

return M
