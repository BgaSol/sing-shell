# Bundled routing data

Four domain sources (cn-domain, Google, GitHub, Telegram) are unchanged SRS files from
SagerNet/sing-geosite, commit 8b8788649349cf73fcd12fc81a5cae580e60360c.
Copyright (C) 2022 nekohasekai <contact-sagernet@sekai.icu>.
They are licensed under GPL-3.0-or-later; LICENSE and COPYING contain the upstream notice and full license.

China IP data is unchanged srs/cn.srs from Loyalsoldier/geoip, release commit
360904642cb991b4b8fa8740cee1320fd2110c72.
Upstream: https://github.com/Loyalsoldier/geoip
Licenses: CC-BY-SA-4.0 and GPL-3.0; see LICENSE-geoip and COPYING-geoip, obtained from
source commit 74979bc45fae8c817081b11c5222863f0f31d4e5.
This product includes GeoLite2 data created by MaxMind, available from https://www.maxmind.com.
Attribution and source data compilation are by Loyalsoldier and the upstream contributors.

manifest.json pins all five source IDs, commits, byte sizes and SHA-256 digests.
source/*.json is editable JSON decompiled from those exact binaries with official sing-box 1.14.4.
No matching data was changed. Rebuild, for example:

    sing-box rule-set compile source/cn-ip.json -o geoip-cn.srs

The SagerNet generator is at https://github.com/SagerNet/sing-geosite.
The IP data generator and its data sources are documented at https://github.com/Loyalsoldier/geoip.
