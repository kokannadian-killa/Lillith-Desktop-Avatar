//! Import an image the way Settings does and write the fitted PNG.
//! `cargo run -p lillith-anim --example avatar_import -- in.jpg out.png`
use lillith_anim::avatar::{import_image, ImportOptions};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (img, meta) = import_image(a[1].as_ref(), ImportOptions::default()).expect("import");
    img.save(&a[2]).expect("save");
    println!("{}", serde_json::to_string_pretty(&meta).unwrap());
}
