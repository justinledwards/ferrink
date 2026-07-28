local InfoMessage = require("ui/widget/infomessage")
local NetworkMgr = require("ui/network/manager")
local Trapper = require("ui/trapper")
local UIManager = require("ui/uimanager")
local WidgetContainer = require("ui/widget/container/widgetcontainer")
local _ = require("gettext")

local FerrinkSync = WidgetContainer:extend{
    name = "ferrinksync",
    is_doc_only = false,
}

local sync_command = "/mnt/us/koreader/plugins/ferrinksync.koplugin/sync-library.sh"

local function trim(value)
    return (value or ""):gsub("^%s+", ""):gsub("%s+$", "")
end

function FerrinkSync:init()
    if self.ui and self.ui.menu then
        self.ui.menu:registerToMainMenu(self)
    end
end

function FerrinkSync:updateLibrary()
    NetworkMgr:runWhenOnline(function()
        Trapper:wrap(function()
            local completed, output = Trapper:dismissablePopen(
                sync_command,
                _("Updating your library…\n\nTap to hide this message. The update will finish safely in the background.")
            )

            if not completed then
                UIManager:show(InfoMessage:new{
                    text = _("The library update is still running. Its private connection will close when it finishes."),
                    timeout = 4,
                })
                return
            end

            output = trim(output)
            if output == "" then
                output = _("The library update did not return a result.")
            end
            UIManager:show(InfoMessage:new{
                text = output,
                timeout = 5,
            })
        end)
    end)
end

function FerrinkSync:addToMainMenu(menu_items)
    menu_items.ferrink_update_library = {
        text = _("Update library"),
        sorting_hint = "more_tools",
        callback = function()
            self:updateLibrary()
        end,
    }
end

return FerrinkSync
