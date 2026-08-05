//! Thin entrypoint. The review loop lives in the library crate.
//!
//! `remendo <change-id>` is the whole interface, but argument handling belongs
//! with the workspace entrypoint (tasks.md 3.1), which must establish that cwd
//! is inside a clone and that the change's project matches it *before* any
//! worktree is created. Until that lands this only reports the version.

fn main() {
    println!("remendo {}", env!("CARGO_PKG_VERSION"));
}
