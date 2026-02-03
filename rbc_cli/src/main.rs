use serde::Deserialize;
use std::fs;
use uuid::Uuid;

// Import the parser module directly to share definitions
#[path = "../../src/rbc.rs"]
mod rbc;

use rbc::{Tag, HEADER_SIZE, RBC_MAGIC, RBC_VERSION};

#[derive(Debug, Deserialize)]
struct Config {
    main: Option<OsConfig>,
    recovery: Option<OsConfig>,
}

#[derive(Debug, Deserialize)]
struct OsConfig {
    uuid: Option<Uuid>,
    fs_type: Option<u16>,
    kernel_params: Option<String>,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <config.toml> <output.rbc>", args[0]);
        std::process::exit(1);
    }

    let input_path = &args[1];
    let output_path = &args[2];

    println!("Reading config from: {}", input_path);

    let toml_content = match fs::read_to_string(input_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error reading {}: {}", input_path, e);
            std::process::exit(1);
        }
    };

    let config: Config = match toml::from_str(&toml_content) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error parsing TOML: {}", e);
            std::process::exit(1);
        }
    };

    let mut atoms: Vec<(Tag, Vec<u8>)> = Vec::new();

    // --- Process Main Section ---
    if let Some(main) = config.main {
        if let Some(uuid) = main.uuid {
            atoms.push((Tag::MainUuid, uuid.as_bytes().to_vec()));
        }
        if let Some(fs_type) = main.fs_type {
            atoms.push((Tag::MainFsType, fs_type.to_le_bytes().to_vec()));
        }
        if let Some(params) = main.kernel_params {
            atoms.push((Tag::MainKernelParams, params.into_bytes()));
        }
    }

    // --- Process Recovery Section ---
    if let Some(rec) = config.recovery {
        if let Some(uuid) = rec.uuid {
            atoms.push((Tag::RecoveryUuid, uuid.as_bytes().to_vec()));
        }
        if let Some(fs_type) = rec.fs_type {
            atoms.push((Tag::RecoveryFsType, fs_type.to_le_bytes().to_vec()));
        }
        if let Some(params) = rec.kernel_params {
            atoms.push((Tag::RecoveryKernelParams, params.into_bytes()));
        }
    }

    // --- Add Signature ---
    // The requirement is to have the signature tag at the end.
    // Since we don't have a real private key signing flow here,
    // we insert a dummy signature to strictly follow the binary format structure.
    let dummy_signature = b"RIGNITE-DEBUG-SIGNATURE";
    atoms.push((Tag::Signature, dummy_signature.to_vec()));

    // --- Serialize to RBC Binary ---
    let mut blob = Vec::new();

    // 1. Magic [u8; 4]
    blob.extend_from_slice(&RBC_MAGIC);

    // 2. Version [u16]
    blob.extend_from_slice(&RBC_VERSION.to_le_bytes());

    // 3. Total Size [u32]
    // Calculate payload size
    let atoms_payload_size: usize = atoms
        .iter()
        .map(|(_, data)| 2 + 2 + data.len()) // Tag(2) + Len(2) + Value
        .sum();

    let total_size = (HEADER_SIZE + atoms_payload_size) as u32;
    blob.extend_from_slice(&total_size.to_le_bytes());

    // 4. Atom Count [u16]
    let atom_count = atoms.len() as u16;
    blob.extend_from_slice(&atom_count.to_le_bytes());

    // 5. Reserved [u8; 4] (Padding to 16 bytes)
    blob.extend_from_slice(&[0u8; 4]);

    // Ensure Header Validity
    assert_eq!(blob.len(), HEADER_SIZE, "Header generation logic failed");

    // 6. Atoms Payload
    for (tag, data) in atoms {
        // Tag
        let tag_val = tag.as_u16();
        blob.extend_from_slice(&tag_val.to_le_bytes());

        // Length
        if data.len() > u16::MAX as usize {
            eprintln!(
                "Error: Atom data too large for tag {:?} (Size: {})",
                tag,
                data.len()
            );
            std::process::exit(1);
        }
        let len = data.len() as u16;
        blob.extend_from_slice(&len.to_le_bytes());

        // Value
        blob.extend_from_slice(&data);
    }

    // --- Write Output ---
    if let Err(e) = fs::write(output_path, &blob) {
        eprintln!("Error writing output file {}: {}", output_path, e);
        std::process::exit(1);
    }

    println!("SUCCESS: Generated RBC file at {}", output_path);
    println!("Stats: {} Atoms, {} Bytes", atom_count, total_size);
}
