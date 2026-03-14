use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::env;
use walkdir::WalkDir;

#[derive(Debug, Serialize, Deserialize, Clone)]
struct TableConfig {
    record_size: u32,
    columns: Vec<Column>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Column {
    name: String,
    field_type: String,
    offset: u32,
    length: u32,
}

type SchemaConfig = BTreeMap<String, TableConfig>;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(dir) = exe_path.parent() {
            let env_path = dir.join(".env");
            dotenvy::from_path(&env_path).ok();
        }
    }

    dotenvy::dotenv().ok();
    
    let base_path = env::var("DB_PATH")
        .map(|v| v.replace('"', ""))
        .unwrap_or_else(|_| {
            let fallback = env::current_dir().unwrap().to_string_lossy().to_string();
            println!("⚠️ AVISO: Variável DB_PATH não encontrada. Usando pasta atual: {}", fallback);
            fallback
        });
    
    let output_file = "schema.toml";
    let mut full_schema: SchemaConfig = BTreeMap::new();

    println!("Varrendo pasta configurada no DB_PATH: {}", base_path);

    for entry in WalkDir::new(&base_path).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        
        if path.extension().and_then(|s| s.to_str()) == Some("dat") {
            let table_name = path.file_stem().unwrap().to_string_lossy().to_string();

            if let Some(table_config) = analyze_dat_file(path).ok().filter(|tc| !tc.columns.is_empty()) {
                println!(
                    "Tabela: {} ({} colunas)",
                    table_name,
                    table_config.columns.len()
                );
                full_schema.insert(table_name, table_config);
            }
        }
    }

    let toml_string = toml::to_string_pretty(&full_schema)?;
    fs::write(output_file, toml_string)?;

    println!(
        "\nPronto! {} tabelas mapeadas no modo reduzido e salvas em {}.",
        full_schema.len(),
        output_file
    );
    Ok(())
}

fn analyze_dat_file(path: &Path) -> std::io::Result<TableConfig> {
    let mut file = File::open(path)?;

    let mut h_info = [0u8; 512];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut h_info)?;

    let total_fields = u16::from_le_bytes(h_info[0x2F..0x31].try_into().unwrap()) as usize;
    let record_size = u16::from_le_bytes(h_info[0x2D..0x2F].try_into().unwrap()) as u32; 

    let mut columns = Vec::new();
    let step = 768;

    let mut fields_buffer = vec![0u8; total_fields * step];
    file.seek(SeekFrom::Start(0x0200))?;
    file.read_exact(&mut fields_buffer)?;

    for i in 0..total_fields {
        let block_start = i * step;
        let block = &fields_buffer[block_start .. block_start + step];

        let name_len = block[2] as usize;
        if name_len > 0 && name_len <= 64 {
            let name_bytes = &block[3..3 + name_len];
            let name = String::from_utf8_lossy(name_bytes)
                .trim_matches(|c: char| c == '\0' || c.is_whitespace()) 
                .to_string();
                
            let type_id = block[0xA4];
            let offset = u16::from_le_bytes(block[0xAC..0xAE].try_into().unwrap()) as u32;
            let length = u16::from_le_bytes(block[0xA6..0xA8].try_into().unwrap()) as u32;

            let type_desc = match type_id {
                1 | 9 => "S",        
                5 | 6 | 0x2E => "I", 
                7 | 8 => "F",        
                2 | 3 | 4 | 11 => "D", 
                _ => "B",            
            };

            if type_id != 0 {
                columns.push(Column {
                    name,
                    field_type: type_desc.to_string(),
                    offset,
                    length,
                });
            }
        }
    }

    columns.sort_by_key(|c| c.offset);

    Ok(TableConfig {
        record_size,
        columns,
    })
}