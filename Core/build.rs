#![allow(non_snake_case)]

use std::io::Write;
use std::path::Path;

fn Header(Output: &mut Vec<u8>, DataSize: u32, Kind: u16, Name: u16, Flags: u16) {
    Output.extend_from_slice(&DataSize.to_le_bytes());
    Output.extend_from_slice(&32u32.to_le_bytes());
    Output.extend_from_slice(&0xFFFFu16.to_le_bytes());
    Output.extend_from_slice(&Kind.to_le_bytes());
    Output.extend_from_slice(&0xFFFFu16.to_le_bytes());
    Output.extend_from_slice(&Name.to_le_bytes());
    Output.extend_from_slice(&0u32.to_le_bytes());
    Output.extend_from_slice(&Flags.to_le_bytes());
    Output.extend_from_slice(&0x0409u16.to_le_bytes());
    Output.extend_from_slice(&0u32.to_le_bytes());
    Output.extend_from_slice(&0u32.to_le_bytes());
}

fn Pad(Output: &mut Vec<u8>) {
    while Output.len() % 4 != 0 {
        Output.push(0);
    }
}

fn Read16(Data: &[u8], Offset: usize) -> u16 {
    u16::from_le_bytes([Data[Offset], Data[Offset + 1]])
}

fn Read32(Data: &[u8], Offset: usize) -> u32 {
    u32::from_le_bytes([Data[Offset], Data[Offset + 1], Data[Offset + 2], Data[Offset + 3]])
}

fn IconResource(Icon: &[u8]) -> Vec<u8> {
    let mut Output = Vec::new();
    Header(&mut Output, 0, 0, 0, 0);
    let Count = Read16(Icon, 4) as usize;
    let mut Group = Vec::new();
    Group.extend_from_slice(&0u16.to_le_bytes());
    Group.extend_from_slice(&1u16.to_le_bytes());
    Group.extend_from_slice(&(Count as u16).to_le_bytes());
    for Index in 0..Count {
        let Entry = 6 + Index * 16;
        let Size = Read32(Icon, Entry + 8) as usize;
        let Offset = Read32(Icon, Entry + 12) as usize;
        let Image = &Icon[Offset..Offset + Size];
        Header(&mut Output, Size as u32, 3, Index as u16 + 1, 0x1010);
        Output.extend_from_slice(Image);
        Pad(&mut Output);
        Group.extend_from_slice(&Icon[Entry..Entry + 8]);
        Group.extend_from_slice(&(Size as u32).to_le_bytes());
        Group.extend_from_slice(&(Index as u16 + 1).to_le_bytes());
    }
    Header(&mut Output, Group.len() as u32, 14, 1, 0x1030);
    Output.extend_from_slice(&Group);
    Pad(&mut Output);
    Output
}

fn main() {
    let Root = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let Manifest = Path::new(&Root).join("Flux.manifest");
    let Icon = Path::new(&Root).join("Assets").join("Flux.ico");
    println!("cargo:rerun-if-changed={}", Manifest.display());
    println!("cargo:rerun-if-changed={}", Icon.display());
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", Manifest.display());
    let Data = std::fs::read(&Icon).expect("Assets/Flux.ico is required");
    let Output = Path::new(&std::env::var("OUT_DIR").unwrap_or_default()).join("Flux.res");
    let mut File = std::fs::File::create(&Output).expect("resource output");
    File.write_all(&IconResource(&Data)).expect("resource write");
    println!("cargo:rustc-link-arg-bins={}", Output.display());
}
