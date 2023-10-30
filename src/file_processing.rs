use std::{error::Error, path::PathBuf, cmp::min};
use tokio::{
    fs::{File, read_to_string},
    io::AsyncWriteExt
};

// Reads the content of a file and returns it as a string
pub async fn read_file(path: &PathBuf) -> Result<String, Box<dyn Error>> {
    read_to_string(path).await.map_err(Into::into)
}

// Writes data to a file in chunks
pub async fn write_in_chunks(file_path: &str, data: &[u8], chunk_size: usize) -> Result<(), Box<dyn Error>> {
    let mut file = File::create(file_path).await?;
    let mut offset = 0;

    while offset < data.len() {
        let end = min(offset + chunk_size, data.len());
        file.write_all(&data[offset..end]).await?;
        offset += chunk_size;
    }

    Ok(())
}