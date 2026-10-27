use compcache::{
    ArtifactWriter, Bypass, Config, Key, ManagedDirectory, QuickCache, Result, StoreBuilder,
};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let root = std::env::args_os()
        .nth(1)
        .expect("usage: cargo run --example basic -- CACHE_PATH");
    let config = Config::from_nickel("config/default.ncl")?;
    let store = StoreBuilder::<QuickCache, ManagedDirectory>::new()
        .path(&root)
        .config(config.clone())
        .open()?;
    let inputs = [*blake3::hash(b"source").as_bytes()];
    let recipe = UserKey {
        compiler: "v1",
        target: "x86_64",
        inputs: &inputs,
        optimization: 2,
        features: &["simd"],
    };
    let key = Key::from_serde("bytecode-v1", &recipe)?;
    let bytecode = store.get_or_insert_blob(key, || Ok(b"compiled bytecode".to_vec()))?;
    println!("bytecode: {} bytes", bytecode.len());

    let runtime_store = StoreBuilder::<Bypass, ManagedDirectory>::new()
        .path(root)
        .config(config)
        .open()?;
    let runtime_inputs = [*blake3::hash(b"runtime source").as_bytes()];
    let runtime_key = Key::from_serde(
        "runtime-v1",
        &UserKey {
            inputs: &runtime_inputs,
            ..recipe
        },
    )?;
    let runtime = runtime_store.get_or_insert_runtime(runtime_key, &|path: &std::path::Path| {
        let mut output = ArtifactWriter::new(path);
        output.write_file("runtime.bin", b"precompiled runtime")?;
        output.finish()
    })?;
    // Retain `runtime` until the compiler/linker finishes using these files.
    println!("runtime: {}", runtime.path().display());
    Ok(())
}

#[derive(serde::Serialize)]
struct UserKey<'a> {
    compiler: &'a str,
    target: &'a str,
    inputs: &'a [[u8; 32]],
    optimization: u8,
    features: &'a [&'a str],
}
