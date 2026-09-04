use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub fn zip_for_upload(entries: Vec<(String, Option<Vec<u8>>)>) -> Result<Vec<u8>> {
    let cursor = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(cursor);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut meta: Vec<serde_json::Value> = Vec::new();

    for (idx, (fname, data)) in entries.into_iter().enumerate() {
        match data {
            Some(bytes) => {
                let name = idx.to_string();
                zip.start_file(&name, opts)?;
                zip.write_all(&bytes)?;
                meta.push(serde_json::json!([fname, name]));
            }
            None => {
                meta.push(serde_json::json!([fname, null]));
            }
        }
    }

    let meta_bytes = serde_json::to_vec(&meta)?;
    zip.start_file("_meta", opts)?;
    zip.write_all(&meta_bytes)?;
    Ok(zip.finish()?.into_inner())
}

pub fn unzip_download(zip_data: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut zip = ZipArchive::new(Cursor::new(zip_data)).context("open media zip")?;
    let fmap: HashMap<String, String> = {
        let mut meta = zip.by_name("_meta").context("media zip missing _meta")?;
        let mut buf = String::new();
        meta.read_to_string(&mut buf)?;
        serde_json::from_str(&buf).context("parse media zip _meta")?
    };

    let mut out = Vec::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        let name = file.name().to_string();
        if name == "_meta" {
            continue;
        }
        let real = fmap
            .get(&name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("zip entry {name} not in _meta"))?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;
        out.push((real, data));
    }
    Ok(out)
}
