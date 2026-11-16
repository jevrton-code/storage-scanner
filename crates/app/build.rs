use embed_manifest::manifest::{
    ActiveCodePage, DpiAwareness, ExecutionLevel, Setting, SupportedOS,
};
use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let manifest = new_manifest("StorageScanner")
            .requested_execution_level(ExecutionLevel::AsInvoker)
            .active_code_page(ActiveCodePage::Utf8)
            .long_path_aware(Setting::Enabled)
            .dpi_awareness(DpiAwareness::PerMonitorV2)
            .supported_os(SupportedOS::Windows10..=SupportedOS::Windows10);
        embed_manifest(manifest).expect("falha ao embutir manifesto");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
