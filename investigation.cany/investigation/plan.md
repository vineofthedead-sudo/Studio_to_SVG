# Plan

1. [done] Analyze program's functionality by finding keywords (e.g., \"shape\", \"color\", \"image\") in its strings to understand core features.
2. [todo] Study the code structure using function addresses/names found via string references or API calls, and document key mechanics in app/MECHANICS.md.
3. [blocked] Create a new Rust project skeleton (Cargo.toml, main.rs) inside app/.
The build failure on eframe/egui indicates that while the dependency is listed, the usage of the framework's API within the `update` function (specifically in drawing components like menus and buttons) is incorrect for this environment or requires deeper structural changes to handle state flow correctly. The next steps must focus on abstracting the core functionality into a single buildable placeholder structure.
4. [todo] Implement the core features identified from the original app's logic into the Rust code, using placeholders for proprietary assets/data where necessary. Must ensure background drawing capability when building an executable with a --screenshot <file.png> option.
Focusing on passing the build check first, and then implementing complex GUI components.
5. [todo] Build and test the new application against the original binary using `rebuild_check` until all differences are accounted for or deemed unresolvable. This build must also run a check that saves a picture to app/shots/now.png when launched with the --screenshot flag.
The focus remains on ensuring the mandatory screenshot mechanism works before tackling GUI fidelity.
