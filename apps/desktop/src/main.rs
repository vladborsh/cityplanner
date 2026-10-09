//! `desktop` app skeleton.

mod app;
mod gallery;

/// The repo's `assets/` directory.
pub const ASSETS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--gallery") {
        gallery::run();
    } else {
        app::run(app::Options::parse(&args));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_builds() {
        assert_eq!(env!("CARGO_PKG_NAME"), "cp_desktop");
    }
}
