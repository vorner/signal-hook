use cc::Build;

fn main() {
    Build::new().file("src/extract.c").compile("extract");
}
