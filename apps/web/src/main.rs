//! `web` app: the same game app as `apps/desktop` (`cp_app::GamePlugin`) on WebGL2.
//!
//! Single-threaded, `SimLimits::wasm()`, assets fetched over HTTP relative to the page
//! (`assets/maps/<map>/...`). Build with `scripts/build-web.sh`; the page is `index.html`.
//! `?map=river_valley` in the URL picks another map (default `flatlands`).

#[cfg(target_arch = "wasm32")]
mod web;

fn main() {
    #[cfg(target_arch = "wasm32")]
    web::run();
    #[cfg(not(target_arch = "wasm32"))]
    println!("cp_web targets wasm32-unknown-unknown; build it with scripts/build-web.sh");
}

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_builds() {
        assert_eq!(env!("CARGO_PKG_NAME"), "cp_web");
    }
}
