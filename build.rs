fn main() {
    println!("cargo:rerun-if-changed=app_icon.rc");
    println!("cargo:rerun-if-changed=res/app.ico");

    embed_resource::compile("app_icon.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("failed to compile Windows resources");
}
