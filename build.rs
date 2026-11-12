// TVN_ITEMCHANGING (the Tree row's checks) and the themed controls are comctl32 v6 only. An app embeds its own manifest
// (res/app.rc) that asks for v6; this covers the SDK's own test and example exes, which have none.
fn main() {
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' \
         version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
    );
}
