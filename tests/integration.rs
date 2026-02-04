//! Integration Tests for Zenith Bundler
//!
//! Verifies the full compilation pipeline using the Rolldown integration.

use std::path::PathBuf;
use zenith_bundler::bundler::create_zenith_bundler;

#[tokio::test]
async fn test_compile_hello_zen() {
    let fixture_path = PathBuf::from("tests/fixtures/hello.zen");
    let cwd = std::env::current_dir().unwrap();
    let absolute_path = cwd.join(fixture_path);
    let entry_str = absolute_path.to_str().unwrap();

    // 1. Create the Bundler
    let mut bundler = create_zenith_bundler(entry_str, None);

    // 2. Generate the bundle (in-memory)
    // bundler.generate() takes no arguments in the current version
    let result = bundler.generate().await;

    match result {
        Ok(outputs) => {
            println!("Bundle generation success!");
            
            // 3. Verify Assets
            for item in outputs.assets {
                // Rolldown output is an enum (Asset or Chunk)
                match item {
                    rolldown_common::Output::Asset(asset) => {
                        println!("Asset: {}", asset.filename);
                        if asset.filename.ends_with(".css") {
                            let content = match &asset.source {
                                rolldown_common::StrOrBytes::Str(s) => s.to_string(),
                                rolldown_common::StrOrBytes::Bytes(b) => String::from_utf8_lossy(b).to_string(),
                            };
                            assert!(content.contains(".hello"), "CSS should contain used classes");
                        }
                    }
                    rolldown_common::Output::Chunk(chunk) => {
                        println!("Chunk: {}", chunk.filename);
                    }
                }
            }
        }
        Err(e) => {
            // It's possible that without proper semantic analysis setup (OXC stuff)
            // or node modules resolution, this might fail.
            // But for a simple .zen file with no imports, it should work.
            panic!("Bundler failed: {:?}", e);
        }
    }
}
