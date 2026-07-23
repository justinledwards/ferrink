-- Apply Ferrink's friendly startup defaults once, through KOReader's supported
-- late user-patch interface. Later choices made in KOReader remain authoritative.

local initialization_key = "ferrink_startup_initialized_v1"
local focus_initialization_key = "ferrink_focus_font_initialized_v1"
local profiles_initialization_key = "ferrink_focus_profiles_initialized_v1"
local focus_font = "Fast Atkinson Hyperlegible"
local normal_font = "Noto Serif"
local focus_font_dir = "/mnt/us/koreader/fonts/ferrink-fast-atkinson"
local focus_font_files = {
    "Fast_Atkinson_Regular.otf",
    "Fast_Atkinson_Bold.otf",
    "Fast_Atkinson_Italic.otf",
    "Fast_Atkinson_BoldItalic.otf",
}

local function is_file(path)
    local descriptor = io.open(path, "rb")
    if not descriptor then
        return false
    end
    descriptor:close()
    return true
end

local function focus_font_is_complete()
    for _, filename in ipairs(focus_font_files) do
        if not is_file(focus_font_dir .. "/" .. filename) then
            return false
        end
    end
    return true
end

local function install_focus_profiles()
    if G_reader_settings:isTrue(profiles_initialization_key) then
        return false
    end

    local DataStorage = require("datastorage")
    local LuaSettings = require("luasettings")
    local profiles = LuaSettings:open(DataStorage:getSettingsDir() .. "/profiles.lua")
    local changed = false

    if not profiles.data["Focus reading"] then
        profiles.data["Focus reading"] = {
            settings = {
                name = "Focus reading",
                order = { "set_font", "font_kerning", "embedded_fonts" },
            },
            set_font = focus_font,
            font_kerning = 3,
            embedded_fonts = false,
        }
        changed = true
    end

    if not profiles.data["Normal reading"] then
        profiles.data["Normal reading"] = {
            settings = {
                name = "Normal reading",
                order = { "set_font", "font_kerning", "embedded_fonts" },
            },
            set_font = normal_font,
            font_kerning = 3,
            embedded_fonts = true,
        }
        changed = true
    end

    if changed then
        profiles:flush()
    end
    G_reader_settings:makeTrue(profiles_initialization_key)
    return true
end

local reader_settings_changed = false

if not G_reader_settings:isTrue(initialization_key) then
    local last_file = G_reader_settings:readSetting("lastfile")
    if last_file and last_file:match("/help/quickstart%-") then
        G_reader_settings:delSetting("lastfile")
    end

    G_reader_settings:saveSetting("start_with", "last")
    G_reader_settings:saveSetting("home_dir", "/mnt/us/documents")
    G_reader_settings:makeTrue(initialization_key)
    reader_settings_changed = true
end

-- Fast Atkinson performs its prefix emphasis through OpenType contextual
-- alternates. KOReader's "best" kerning mode selects full HarfBuzz shaping,
-- which is required for those substitutions. Apply this once only; later font
-- and typesetting choices made by the reader remain authoritative.
local focus_font_available = focus_font_is_complete()
if focus_font_available and not G_reader_settings:isTrue(focus_initialization_key) then
    G_reader_settings:saveSetting("cre_font", focus_font)
    G_reader_settings:saveSetting("copt_font_kerning", 3)
    G_reader_settings:saveSetting("copt_embedded_fonts", 0)
    G_reader_settings:makeTrue("cre_font_family_ignore_font_names")
    G_reader_settings:makeTrue(focus_initialization_key)
    reader_settings_changed = true
end

if focus_font_available and install_focus_profiles() then
    reader_settings_changed = true
end

-- Persist before the UI starts. A supervised app may be interrupted before
-- KOReader's normal shutdown flush, and no one-time initialization should be
-- repeated or lost in that case. Avoid rewriting settings on later launches.
if reader_settings_changed then
    G_reader_settings:flush()
end
