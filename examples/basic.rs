use compcache::{
    ArtifactWriter, Bypass, Config, Key, ManagedDirectory, QuickCache, Result, SchemaKey,
    StoreBuilder,
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
    let source = blake3::hash(b"source").to_hex();
    let recipe = UserKey {
        compiler: "v1",
        target: "x86_64",
        source: source.as_str(),
        flags: "-O2",
    };
    let key = Key::new(&recipe);
    let bytecode = store.get_or_insert_blob(key, || Ok(b"compiled bytecode".to_vec()))?;
    println!("bytecode: {} bytes", bytecode.len());

    let runtime_store = StoreBuilder::<Bypass, ManagedDirectory>::new()
        .path(root)
        .config(config)
        .open()?;
    let runtime_key = Key::new(&UserKey {
        source: "runtime-source-digest",
        ..recipe
    });
    let runtime = runtime_store.get_or_insert_runtime(runtime_key, &|path: &std::path::Path| {
        let mut output = ArtifactWriter::new(path);
        output.write_file("runtime.bin", b"precompiled runtime")?;
        output.finish()
    })?;
    // Retain `runtime` until the compiler/linker finishes using these files.
    println!("runtime: {}", runtime.path().display());
    Ok(())
}

struct UserKey<'a> {
    compiler: &'a str,
    target: &'a str,
    source: &'a str,
    flags: &'a str,
}
impl SchemaKey<4> for UserKey<'_> {
    const KEY_FIELDS: [&'static str; 4] = ["compiler", "target", "source", "flags"];
    fn key_value(&self) -> [&str; 4] {
        [self.compiler, self.target, self.source, self.flags]
    }
}
