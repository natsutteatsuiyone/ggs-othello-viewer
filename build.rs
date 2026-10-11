fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/ggs-icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile_for("assets/app.rc", ["ggs-othello-viewer"], embed_resource::NONE)
            .manifest_required()
            .expect("failed to embed the application icon");
    }
}
