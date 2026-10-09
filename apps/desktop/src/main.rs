//! `desktop` app skeleton.

fn main() {
    println!("cityplanner desktop: skeleton");
}

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_builds() {
        assert_eq!(env!("CARGO_PKG_NAME"), "cp_desktop");
    }
}
