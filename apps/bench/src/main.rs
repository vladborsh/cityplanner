//! `bench` app skeleton.

fn main() {
    println!("cityplanner bench: skeleton");
}

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_builds() {
        assert_eq!(env!("CARGO_PKG_NAME"), "cp_bench");
    }
}
