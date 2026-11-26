# Bundled site rules

Source: https://github.com/SagerNet/sing-geosite/tree/8b8788649349cf73fcd12fc81a5cae580e60360c

Copyright (C) 2022 by nekohasekai <contact-sagernet@sekai.icu>
License: GNU GPL version 3 or later. The upstream notice is in LICENSE;
the complete license is in COPYING. No rule content was changed.

manifest.json identifies every binary and its SHA-256. source/*.json contains
editable JSON reconstructed from those exact SRS files with official sing-box
1.14.4 `rule-set decompile`. Rebuild each file with:

    sing-box rule-set compile source/github.json -o geosite-github.srs

Replace github with the other site identifiers. JSON, binaries, manifest and
license texts are included in the application resources and source repository.
The upstream generator is at https://github.com/SagerNet/sing-geosite.
