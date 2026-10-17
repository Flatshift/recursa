// Re-embed the UI whenever the built assets change.
fn main() {
    println!("cargo:rerun-if-changed=../../web/dist");
}
