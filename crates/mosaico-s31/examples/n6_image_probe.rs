//! Independent externally supplied ESP image corpus through production parsing.
use mosaico_s31::ota::{image::ImageVerifier, parse_sha};
use sha2::{Digest, Sha256};
use std::{env, fs, path::Path};

fn main() {
    let args: Vec<_> = env::args().collect();
    assert_eq!(args.len(), 2, "n6_image_probe <corpus-directory>");
    let root = Path::new(&args[1]);
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("receipt.json")).unwrap()).unwrap();
    let mut cases = 0;
    for case in receipt["cases"].as_array().unwrap() {
        let name = case["file"].as_str().unwrap();
        let bytes = fs::read(root.join(name)).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            case["sha256"].as_str().unwrap(),
            "frozen fixture changed: {name}"
        );
        for chunk in [1, 17, 4096] {
            let mut verifier = ImageVerifier::new(bytes.len() as u32);
            let result = bytes
                .chunks(chunk)
                .try_for_each(|block| verifier.feed(block))
                .and_then(|_| verifier.finish());
            assert_eq!(
                result.is_ok(),
                case["expect"] == "accept",
                "case={name} chunk={chunk}: {result:?}"
            );
            if result.is_ok() {
                let expected_marker = name != "legacy-layout-marker.bin"
                    && receipt["source"]["ab_layout"].as_bool().unwrap_or(false);
                assert_eq!(verifier.ab_layout(), expected_marker, "marker {name}");
            }
        }
        cases += 1;
        println!("PASS {name} {} (chunks 1/17/4096)", case["expect"]);
    }
    // Text from a USB command must not panic on a UTF-8 slicing boundary.
    for text in ["é".repeat(32), "0".repeat(63) + "g", "0".repeat(65)] {
        assert!(parse_sha(&text).is_err());
    }
    println!("IMAGE_CORPUS_COMPLETE cases={cases}");
}
