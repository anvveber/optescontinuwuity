//! Embeds the files in `pages/resources` (crate memory-serve).
//!
//! memory-serve writes each file's absolute source path into the generated
//! code. When the files are embedded (release builds), that path is never read,
//! but it still ends up in the binary and names the directory the server was
//! built in. In that case it is replaced by the path relative to this crate.
//! Builds that load the files from disk at runtime (debug builds) keep the
//! absolute path, because they need it.

use std::{env, fs, path::PathBuf};

const RESOURCES: &str = "./pages/resources";

fn main() {
	memory_serve::load_directory(RESOURCES);
	strip_source_paths();
}

fn strip_source_paths() {
	let generated = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by cargo"))
		.join("memory_serve_assets.rs");
	let code = fs::read_to_string(&generated).expect("memory-serve wrote its asset file");

	// Files not embedded: the server reads them from `path` at runtime.
	if !code.contains("bytes: Some(") {
		return;
	}

	// Only the `path` field. The `include_bytes!` lines need the real path to
	// compile, and do not end up in the binary.
	let dir = fs::canonicalize(RESOURCES).expect("the resources directory exists");
	let absolute = format!("path: r\"{}", dir.to_string_lossy());
	assert!(
		code.contains(&absolute),
		"memory-serve's generated code changed; build paths would end up in the binary. Check \
		 src/web/build.rs."
	);
	let code = code.replace(&absolute, "path: r\"pages/resources");

	fs::write(&generated, code).expect("asset file can be rewritten");
}
