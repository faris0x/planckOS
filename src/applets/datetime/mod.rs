use crate::hal::display::VgaDisplay;
use crate::applets::Applet;

pub static APPLET: Applet = Applet {
    name: "datetime",
    description: "Display current date and time",
    run: run,
};


use crate::hal::input::Ps2Keyboard;
use crate::hal::rtc;
use crate::hal::Display;

pub fn run(display: &mut VgaDisplay, _input: &mut Ps2Keyboard, _args: &[&str]) {
    let dt = rtc::read_datetime();

    let months = [
        "January", "February", "March", "April", "May", "June",
        "July", "August", "September", "October", "November", "December",
    ];
    let month_name = if (dt.month as usize) <= 12 {
        months[(dt.month - 1) as usize]
    } else {
        "???"
    };

    let mut buf = [0u8; 16];

    display.write(month_name);
    display.write(" ");
    display.write(fmt_num(dt.day as u32, &mut buf));
    display.write(", ");
    display.write(fmt_num(dt.year as u32, &mut buf));
    display.write("  ");
    display.write(fmt_two(dt.hours, &mut buf));
    display.write(":");
    display.write(fmt_two(dt.minutes, &mut buf));
    display.write(":");
    display.writeln(fmt_two(dt.seconds, &mut buf));
}

fn fmt_num<'a>(n: u32, buf: &'a mut [u8; 16]) -> &'a str {
    let mut i = 15;
    let mut v = n;
    loop {
        i -= 1;
        buf[i] = (v % 10) as u8 + b'0';
        v /= 10;
        if v == 0 || i == 0 { break; }
    }
    let len = 15 - i;
    core::str::from_utf8(&buf[i..i + len]).unwrap_or("?")
}

fn fmt_two<'a>(n: u8, buf: &'a mut [u8; 16]) -> &'a str {
    buf[0] = b'0' + (n / 10);
    buf[1] = b'0' + (n % 10);
    core::str::from_utf8(&buf[..2]).unwrap_or("??")
}