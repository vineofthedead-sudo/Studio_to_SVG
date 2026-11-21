# What we found in /home/sai/Projects/Silhouette Studio/SS_V5.0.556.001_W6R.exe

SS_V5.0.556.001_W6R.exe is a program for Windows, built for x86 (32-bit). Its internal names were removed (stripped), so most functions will have generated names.

Evidence ev_c4060d83e7b6dfc713130bf7041fb25d3e276a93c2223b98cd2c73304b72c89d.

```json
{
  "architecture": "x86",
  "bits": 32,
  "dotnet_arch": null,
  "entry_point": "0x4b01b9",
  "format": "pe",
  "format_name": "PE (Windows)",
  "kind": "executable",
  "libraries": [
    "ADVAPI32.dll",
    "COMCTL32.dll",
    "COMDLG32.dll",
    "GDI32.dll",
    "KERNEL32.dll",
    "MSIMG32.dll",
    "NETAPI32.dll",
    "OLEAUT32.dll",
    "SHELL32.dll",
    "SHLWAPI.dll",
    "USER32.dll",
    "VERSION.dll",
    "dbghelp.dll",
    "ole32.dll",
    "msi.dll",
    "gdiplus.dll",
    "UxTheme.dll",
    "WININET.dll",
    "dwmapi.dll"
  ],
  "next_steps": [
    {
      "tool": "core_overview_show",
      "why": "a one-page summary: sections, imports, counts"
    },
    {
      "tool": "native_functions_list",
      "why": "every function we found"
    },
    {
      "tool": "native_strings_find",
      "why": "find text the program shows or uses"
    },
    {
      "tool": "native_function_study",
      "why": "a full dossier on one function (start with main or the entry point)"
    },
    {
      "tool": "core_feature_find",
      "why": "follow a word you see in the app to the code behind it"
    },
    {
      "tool": "native_pe_resources_list",
      "why": "icons, version information and the manifest"
    }
  ],
  "operating_system": "Windows",
  "plain_english": "SS_V5.0.556.001_W6R.exe is a program for Windows, built for x86 (32-bit). Its internal names were removed (stripped), so most functions will have generated names.",
  "recommended_toolsets": [
    "core",
    "native",
    "formats"
  ],
  "stripped": true,
  "symbol_table_stripped": true,
  "toolchain_hints": [
    {
      "category": "compiler",
      "evidence": "Rich header",
      "id": "msvc",
      "label": "Microsoft Visual C++ toolchain",
      "strength": "strong"
    }
  ]
}
```

## Ghidra

Deep mode: Ghidra has analysed /home/sai/Projects/Silhouette Studio/SS_V5.0.556.001_W6R.exe (1 min), so decompiling it is quick.
