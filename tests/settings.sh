#!/bin/sh
# Write the settings file the UI tests start WeekNumber with:
#   tests/settings.sh <settings.json> <light|dark>
# Public holidays of the Netherlands and Germany and a second clock for
# Tokyo, so the calendar shows everything it can; regular digits (the
# Linux default).
set -eu
dark=false
[ "$2" = dark ] && dark=true
mkdir -p "$(dirname "$1")"
cat > "$1" <<JSON
{
    "SelectedColor": "#FFFFFF",
    "IsBold": false,
    "HolidayCountry": "NL",
    "HolidayCountryPrimary": "NL",
    "HolidayCountrySecondary": "DE",
    "DarkMode": $dark,
    "StartupInitialized": true,
    "TimeZoneEnabled": true,
    "TimeZoneKey": "Tokyo Standard Time"
}
JSON
