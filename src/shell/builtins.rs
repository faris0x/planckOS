use crate::hal::display::VgaDisplay;
use crate::hal::Display;
use crate::applets::AppletRegistry;

fn print_cmd(display: &mut VgaDisplay, name: &str, desc: &str) {
    display.write("  ");
    display.write(name);
    let pad_len = 12usize.saturating_sub(name.len());
    for _ in 0..pad_len {
        display.putchar(b' ');
    }
    display.write("- ");
    display.writeln(desc);
}

pub fn print_help(display: &mut VgaDisplay, registry: &AppletRegistry) {
    display.writeln("Built-in commands:");
    print_cmd(display, "echo", "Prints text");
    print_cmd(display, "cls", "Clears screen");
    print_cmd(display, "banner", "Shows welcome banner");
    print_cmd(display, "history", "Shows command history");
    print_cmd(display, "ls", "List directory contents");
    print_cmd(display, "out", "Display file contents");
    print_cmd(display, "mk", "Create files and directories");
    print_cmd(display, "rm", "Remove files and directories");
    print_cmd(display, "cp", "Copy files and directories");
    print_cmd(display, "shutdown", "Shuts down the system");
    print_cmd(display, "help", "This message");
    display.writeln("");

    // List registered applets
    if registry.list().next().is_some() {
        display.writeln("Applets:");
        for applet in registry.list() {
            print_cmd(display, applet.name, applet.description);
        }
    }
}