//! 从 32 字节 Ed25519 seed 文件推导公钥（发布流程辅助工具）。
use ed25519_dalek::SigningKey;

fn main() {
    let path = std::env::args().nth(1).expect("usage: derive_pubkey <seed-file>");
    let text = std::fs::read_to_string(&path).expect("read seed file");
    let mut seed = [0_u8; 32];
    assert!(text.trim().len() == 64, "seed must be 64 hex chars");
    for (i, chunk) in text.trim().as_bytes().chunks(2).enumerate() {
        seed[i] = u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).expect("seed must be hex");
    }
    let signing = SigningKey::from_bytes(&seed);
    for byte in signing.verifying_key().as_bytes() {
        print!("{byte:02x}");
    }
    println!();
}
