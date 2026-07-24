-- Apply Ferrink's friendly startup defaults once, through KOReader's supported
-- late user-patch interface. Later choices made in KOReader remain authoritative.

local initialization_key = "ferrink_startup_initialized_v1"
local typography_migration_key = "ferrink_typography_migrated_v2"
local profiles_initialization_key = "ferrink_focus_profiles_initialized_v1"
local reading_font = "Noto Serif"

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
            set_font = reading_font,
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
            set_font = reading_font,
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

local function migrate_typography()
    if G_reader_settings:isTrue(typography_migration_key) then
        return false
    end

    G_reader_settings:saveSetting("cre_font", reading_font)
    G_reader_settings:saveSetting("copt_font_kerning", 3)
    G_reader_settings:saveSetting("copt_embedded_fonts", 0)
    G_reader_settings:delSetting("cre_font_family_ignore_font_names")

    local DataStorage = require("datastorage")
    local LuaSettings = require("luasettings")
    local profiles = LuaSettings:open(DataStorage:getSettingsDir() .. "/profiles.lua")
    for _, profile_name in ipairs({ "Focus reading", "Normal reading" }) do
        local profile = profiles.data[profile_name]
        if profile then
            profile.set_font = reading_font
            profile.embedded_fonts = false
        end
    end
    profiles:flush()

    G_reader_settings:makeTrue(typography_migration_key)
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

-- Use the conventional Noto family for ordinary, comfortable reading. Fast
-- Atkinson remains installed as an opt-in local choice, but its deliberate
-- prefix emphasis is not a good default for headings, quotes, or long books.
if migrate_typography() then
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
