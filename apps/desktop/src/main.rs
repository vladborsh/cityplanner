//! `desktop` app skeleton.

mod gallery;

fn main() {
    if std::env::args().any(|a| a == "--gallery") {
        gallery::run();
        return;
    }
    println!("cityplanner desktop: skeleton");
}

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_builds() {
        assert_eq!(env!("CARGO_PKG_NAME"), "cp_desktop");
    }
}
