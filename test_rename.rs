use std::fs;

fn main() {
    fs::write("test_src.bin", b"src").unwrap();
    fs::write("test_dst.bin", b"dst").unwrap();
    match fs::rename("test_src.bin", "test_dst.bin") {
        Ok(_) => println!("rename succeeded"),
        Err(e) => println!("rename failed: {}", e),
    }
}
