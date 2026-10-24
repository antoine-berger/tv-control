# Android resource-table test fixture

`aosp-basic.arsc` is the unmodified `resources.arsc` member extracted from AOSP's
`libs/androidfw/tests/data/basic/basic.apk`. No APK code was executed.

- Upstream: https://android.googlesource.com/platform/frameworks/base/+/master/libs/androidfw/tests/data/basic/basic.apk
- APK Git blob: `b721ebfde4459a75a626a452c8fd573614469bfb`
- Extracted table SHA-256: `fe996e5dca40ae10d60e2e83721d819c7f9e701757c5b6516b35429935e1049c`
- Retrieved 2026-10-23.
- Copyright (C) 2014 The Android Open Source Project; Apache License 2.0,
  reproduced in `LICENSE-2.0.txt`.

The upstream `basic/build` script generates this APK using `aapt2 compile` and
`aapt2 link`. Source `basic/res/values/values.xml` declares string resource
`0x7f030000` as `test1` and `0x7f030001` as `test2`; `basic/R.h` confirms these IDs.
The fixture validates the real compiler format independently of the handmade
malformed/variant fixtures in the unit tests. It is test data, not a deployed
application or proof of behavior on a live TV.

## Synthetic media-session dump

`media-session.txt` is a synthetic, non-device fixture with invented identity and
metadata. Its record header/fields follow AOSP `MediaSessionRecord.dump` and
`toString` (source blob `5f7c86f7b670dfbd42e68882f98c4de01b4a7f48`):
https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/services/core/java/com/android/server/media/MediaSessionRecord.java

Targeted monitor command support was checked against AOSP `MediaShellCommand`
(source blob `19f16cc3800090d8099cd7b2281293b6d4cf9648`):
https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/services/core/java/com/android/server/media/MediaShellCommand.java

These sources were inspected on 2026-10-24. The monitor accepts play, pause, next,
previous and quit, not stop or seek. The record dump includes playback position
and a metadata description, not reliable duration/media-ID fields. This fixture
qualifies parsing of that text format only, not live TV behavior or media delivery.
