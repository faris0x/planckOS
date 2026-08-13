#[cfg(applet_sysinfo)]
pub mod sysinfo;
#[cfg(applet_datetime)]
pub mod datetime;
pub mod heap;
pub mod ls;
pub mod out;
pub mod mk;
pub mod rm;
pub mod cp;

use crate::hal::input::Ps2Keyboard;
use crate::hal::Display;

/// A registered applet that can be invoked from the shell.
#[derive(Clone, Copy)]
pub struct Applet {
    pub name: &'static str,
    pub description: &'static str,
    pub run: fn(&mut dyn Display, &mut Ps2Keyboard, &[&str]),
}

/// Registry of all compiled-in applets.
pub struct AppletRegistry {
    applets: &'static [Applet],
}

impl AppletRegistry {
    pub const fn new(applets: &'static [Applet]) -> Self {
        AppletRegistry { applets }
    }

    pub fn list(&self) -> core::slice::Iter<'_, Applet> {
        self.applets.iter()
    }

    pub fn find(&self, name: &str) -> Option<&Applet> {
        self.applets.iter().find(|a| a.name == name)
    }
}

/// List of all registered applets, built at compile time.
/// Add new applets here as they are implemented.
static APPLET_LIST: &[Applet] = &[
    #[cfg(applet_sysinfo)]
    sysinfo::APPLET,
    #[cfg(applet_datetime)]
    datetime::APPLET,
    heap::APPLET,
];

/// Build the applet registry from all enabled applet modules.
pub fn build_registry() -> AppletRegistry {
    AppletRegistry::new(APPLET_LIST)
}
