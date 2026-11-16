//! Teste de integração MFT × Walker em um VHD NTFS temporário.
//!
//! Exige administrador (diskpart + leitura bruta do volume):
//! `cargo test -p ss-scan --test mft_vhd -- --ignored --nocapture`

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

use ss_core::Tree;
use ss_scan::mft::{self, Labels};
use ss_scan::walk::WalkScanner;
use ss_scan::{Progress, ScanTarget, Scanner};

fn diskpart(script: &str, dir: &Path) {
    let file = dir.join("script.txt");
    std::fs::write(&file, script).unwrap();
    let out = Command::new("diskpart")
        .arg("/s")
        .arg(&file)
        .output()
        .expect("diskpart");
    assert!(
        out.status.success(),
        "diskpart falhou: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

fn free_letter() -> char {
    ('R'..='Z')
        .rev()
        .find(|c| !Path::new(&format!("{c}:\\")).exists())
        .expect("sem letra livre")
}

struct Vhd {
    dir: PathBuf,
    file: PathBuf,
    letter: char,
}

impl Vhd {
    fn create() -> Self {
        let dir = std::env::temp_dir().join("ss_vhd_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("t.vhd");
        let letter = free_letter();
        diskpart(
            &format!(
                "create vdisk file=\"{}\" maximum=128 type=expandable\nattach vdisk\ncreate partition primary\nformat fs=ntfs quick label=SSTEST\nassign letter={letter}\n",
                file.display()
            ),
            &dir,
        );
        Self { dir, file, letter }
    }
    fn root(&self) -> String {
        format!("{}:\\", self.letter)
    }
}

impl Drop for Vhd {
    fn drop(&mut self) {
        diskpart(
            &format!(
                "select vdisk file=\"{}\"\ndetach vdisk\n",
                self.file.display()
            ),
            &self.dir,
        );
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn populate(root: &str) {
    let data = Path::new(root).join("data");
    std::fs::create_dir_all(data.join("a/b/c")).unwrap();
    std::fs::create_dir_all(data.join("compressed")).unwrap();
    std::fs::write(data.join("tiny.txt"), b"hello").unwrap(); // residente
    std::fs::write(data.join("a/mid.bin"), vec![7u8; 300_000]).unwrap();
    std::fs::write(data.join("a/b/big.bin"), vec![9u8; 6_000_000]).unwrap();
    // Nomes unicode e longos.
    std::fs::write(data.join("a/b/c/ação ñ ✓.dat"), vec![1u8; 70_000]).unwrap();
    let long = "x".repeat(200);
    std::fs::write(data.join("a").join(format!("{long}.log")), vec![2u8; 10]).unwrap();
    // Muitos arquivos pequenos forçam MFT com vários registros/extensões.
    for i in 0..3000 {
        std::fs::write(data.join(format!("a/b/f{i:05}.t")), vec![3u8; i % 5000]).unwrap();
    }
    // Compressão NTFS.
    std::fs::write(data.join("compressed/zeros.bin"), vec![0u8; 2_000_000]).unwrap();
    let _ = Command::new("compact")
        .args([
            "/c",
            "/q",
            &data.join("compressed\\zeros.bin").to_string_lossy(),
        ])
        .output();
}

#[test]
#[ignore = "exige administrador (diskpart + leitura bruta do volume)"]
fn mft_matches_walker_on_vhd() {
    assert!(
        ss_scan::elevation::is_elevated(),
        "execute como administrador"
    );
    let vhd = Vhd::create();
    populate(&vhd.root());

    let (p1, p2) = (Progress::default(), Progress::default());
    let cancel = AtomicBool::new(false);
    let drive = format!("{}:", vhd.letter);
    let t0 = std::time::Instant::now();
    let mft_tree = mft::scan_volume(&drive, &vhd.root(), &Labels::default(), &p1, &cancel).unwrap();
    let mft_time = t0.elapsed();
    let t1 = std::time::Instant::now();
    let walk_tree = WalkScanner::local()
        .scan(
            &ScanTarget::Path(PathBuf::from(format!("{}data", vhd.root()))),
            &p2,
            &cancel,
        )
        .unwrap();
    let walk_time = t1.elapsed();
    println!("MFT {mft_time:?} | Walker {walk_time:?}");

    let id = mft_tree
        .find_child_path(Tree::ROOT, &["data"])
        .expect("pasta data na MFT");
    let (m, w) = (mft_tree.node(id), walk_tree.node(Tree::ROOT));
    assert_eq!(m.files, w.files, "contagem de arquivos");
    assert_eq!(m.dirs, w.dirs, "contagem de pastas");
    assert_eq!(m.size, w.size, "tamanho lógico");
    // O alocado deve coincidir (a MFT conta arquivos residentes como 0, como o walker).
    assert_eq!(m.allocated, w.allocated, "tamanho alocado");
}
