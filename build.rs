fn main() {
    let bindings = bindgen::builder()
        .header_contents(
            "bindings.h",
            "
                         #include <linux/input.h>
                         #include <linux/input-event-codes.h>
                         ",
        )
        .generate()
        .expect("Failed to generate bindindgs");

    bindings
        .write_to_file(format!(
            "{}/input_bindings.rs",
            std::env::var("OUT_DIR").expect("OUT_DIR not set")
        ))
        .expect("Failed to write bindings");
}
