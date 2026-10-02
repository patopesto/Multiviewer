# NDI runtime (vendored)

`task setup` downloads and verifies the right SDK for your OS into `<os>/sdk/`
— prefer that. The instructions below are for installing manually.

The NDI runtime is proprietary and cannot be committed to this repo.
Download the NDI SDK (https://ndi.video/ → "NDI SDK", license acceptance required)
and drop the runtime libraries here:

- `macos/`   → `libndi.dylib`
- `windows/` → `Processing.NDI.Lib.x64.dll` (and any DLLs it ships with)
- `linux/`   → `libndi.so` + its versioned files (`libndi.so.6*`), keep symlinks

The package scripts bundle these into the app; the app loads NDI from the
bundle at startup (Phase 2), never from a system install.

Shipping the runtime requires complying with the NDI SDK license (attribution).
