use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
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
}

type SchemaConfig = BTreeMap<String, TableConfig>;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root_path = std::env::current_dir()?;
    let output_file = "schema.toml";
    let mut full_schema: SchemaConfig = BTreeMap::new();

    println!("Varrendo pasta: {:?}", root_path);

    for entry in WalkDir::new(&root_path).into_iter().filter_map(|e| e.ok()) {
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
        "\nPronto! {} tabelas mapeadas no modo reduzido.",
        full_schema.len()
    );
    Ok(())
}

fn analyze_dat_file(path: &Path) -> std::io::Result<TableConfig> {
    let mut file = File::open(path)?;

    let mut header = vec![0u8; 64 * 1024];
    let n = file.read(&mut header)?;
    header.truncate(n);

    let mut columns = Vec::new();
    let mut current_pos = 0x0200;
    let step = 768;

    while current_pos + step <= header.len() {
        let block = &header[current_pos..current_pos + step];

        let name_len = block[2] as usize;
        if name_len > 0 && name_len <= 64 {
            let name_bytes = &block[3..3 + name_len];
            if name_bytes
                .iter()
                .all(|&b| b.is_ascii_graphic() || b == b' ')
            {
                let name = String::from_utf8_lossy(name_bytes).to_string();
                let type_id = block[0xA4];

                let offset = (u16::from_le_bytes(block[0xAC..0xAE].try_into().unwrap()) as u32) + 1;

                let type_desc = match type_id {
                    1 => "S",
                    6 | 0x2E => "I", // 7430 em hex costuma aparecer como bytes específicos
                    7 => "F",
                    2 | 11 => "D",
                    _ => "B",
                };

                if type_id != 0 && offset > 1 {
                    columns.push(Column {
                        name,
                        field_type: type_desc.to_string(),
                        offset,
                    });
                }
            }
        }
        current_pos += step;
    }

    columns.sort_by_key(|c| c.offset);

    let mut h_info = [0u8; 512];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut h_info)?;
    let record_size = (u16::from_le_bytes(h_info[0x2D..0x2F].try_into().unwrap()) as u32) + 1; // flag de deletado ou não

    Ok(TableConfig {
        record_size,
        columns,
    })
}
