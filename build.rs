use std::path::PathBuf;

fn generate_bindings(input_file_content: &str, output_name: &str, extra_includes: &[PathBuf]) {
    let cflags = extra_includes.iter().map(|v| format!("-I{}", v.display()));

    let bindings = bindgen::builder()
        .header_contents("bindings.h", input_file_content)
        .clang_args(cflags)
        .generate()
        .expect("Failed to generate bindindgs");

    bindings
        .write_to_file(format!(
            "{}/{}",
            std::env::var("OUT_DIR").expect("OUT_DIR not set"),
            output_name,
        ))
        .expect("Failed to write bindings");
}

fn generate_input_bindings() {
    generate_bindings(
        "
                      #include <linux/input.h>
                      #include <linux/input-event-codes.h>
                      ",
        "input_bindings.rs",
        &[],
    );
}

fn generate_xkb_bindings(xkb_includes: &[PathBuf]) {
    generate_bindings(
        "
                      #include <xkbcommon/xkbcommon.h>
                      ",
        "xkb_bindings.rs",
        xkb_includes,
    );
}

fn main() {
    let library = pkg_config::probe_library("xkbcommon").expect("Failed to find xkbcommon");
    generate_input_bindings();
    generate_xkb_bindings(&library.include_paths);
}
