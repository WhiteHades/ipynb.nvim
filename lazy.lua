return {
  {
    "WhiteHades/ipynb.nvim",
    lazy = false,
    main = "ipynb",
    opts = {},
    init = function(plugin)
      dofile(plugin.dir .. "/lua/ipynb/init.lua").init(require("lazy.core.plugin").values(plugin, "opts", false))
    end,
    build = function(plugin)
      vim.opt.rtp:append(plugin.dir)
      require("ipynb").install()
    end,
    dependencies = {
      { "goerz/jupytext.nvim", version = "0.2.0" },
      { "quarto-dev/quarto-nvim", dependencies = { "jmbuhr/otter.nvim" } },
      { "nvim-treesitter/nvim-treesitter", build = ":TSUpdate", opts = function(_, opts)
        opts.ensure_installed = opts.ensure_installed or {}
        for _, parser in ipairs({ "markdown", "markdown_inline", "python" }) do
          if not vim.tbl_contains(opts.ensure_installed, parser) then table.insert(opts.ensure_installed, parser) end
        end
      end },
      { "3rd/image.nvim", opts = { processor = "magick_cli", scale_factor = 2.0,
        max_width_window_percentage = 95, max_height_window_percentage = 85,
        window_overlap_clear_enabled = false,
        integrations = { markdown = { only_render_image_at_cursor = false,
          only_render_image_at_cursor_mode = "inline", floating_windows = true } },
      } },
    },
  },
  { "lewis6991/gitsigns.nvim", optional = true, opts = function(_, opts)
    local previous = opts.on_attach
    opts.on_attach = function(buf)
      if vim.api.nvim_buf_get_name(buf):match("%.ipynb$") then return false end
      if previous then return previous(buf) end
    end
  end },
}
