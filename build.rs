//! Puts the app's icon on the .exe itself, so it is the note in Explorer
//! and not a blank Windows default.
//!
//! This is the only thing a build script is here for. On anything other
//! than Windows it does nothing at all, and if the resource compiler is
//! missing it says so and carries on: an icon is not worth a failed build.

fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/icon.ico");

        let mut resource = winresource::WindowsResource::new();

        resource.set_icon("assets/icon.ico");

        // `rc.exe` comes with the Windows SDK, which Visual Studio Build
        // Tools installs. Without it the app still builds and runs, it
        // just keeps the default icon on the file
        if let Err(error) = resource.compile() {
            println!("cargo:warning=could not put the icon on the .exe: {error}");
        }
    }
}
