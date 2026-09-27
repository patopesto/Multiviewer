# Generated DeckLink Windows headers

The files in this directory are generated from the Blackmagic DeckLink SDK IDL
files using the Microsoft IDL compiler (`midl.exe`). They are committed so that
Windows builds of the `multiviewer-decklink` crate can compile without requiring
MIDL on every machine.

## Regenerating

The task requires `midl.exe` (Windows SDK) and `cl.exe` (MSVC) in PATH. There
are two ways to provide these:

### From a Developer Command Prompt

Open **x64 Native Tools Command Prompt for VS 2022** (or the equivalent for
your VS version) and run:

```bash
task windows:generate:decklink-headers
```

### From a regular shell with `VCVARS_PATH`

If you are not in a Developer Command Prompt, set `VCVARS_PATH` to your
`vcvars64.bat` so MIDL can find `cl.exe`:

```bash
export VCVARS_PATH="C:/Program Files/Microsoft Visual Studio/2022/Community/VC/Auxiliary/Build/vcvars64.bat"
task windows:generate:decklink-headers
```

Common `vcvars64.bat` locations:

```text
C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat
C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Auxiliary\Build\vcvars64.bat
C:\Program Files\Microsoft Visual Studio\2022\Professional\VC\Auxiliary\Build\vcvars64.bat
```

### Overriding the MIDL path

The default `midl.exe` path is:

```text
C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\midl.exe
```

Set `MIDL_PATH` to override:

```bash
export MIDL_PATH="C:/Program Files (x86)/Windows Kits/10/bin/<your-version>/x64/midl.exe"
task windows:generate:decklink-headers
```

The generated `DeckLinkAPI.h` and `DeckLinkAPI_i.c` preserve the BSD-style
license header from the upstream `.idl` files and may be committed under the
terms of the Blackmagic Design SDK license.
