extern crate cpp_build;

fn main() {
    let mut conf = cpp_build::Config::new();
    conf.include("C:\\Python37\\include");
    conf.build("src/lib.rs");
}
