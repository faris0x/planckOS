/// CMOS RTC driver — reads date and time from the real-time clock.
///
/// Ports:
///   0x70  — CMOS index register (bit 7 = NMI disable)
///   0x71  — CMOS data register

const CMOS_IDX: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

// ── CMOS register indices ───────────────────────────────────────

const RTC_SECONDS: u8 = 0x00;
const RTC_MINUTES: u8 = 0x02;
const RTC_HOURS:   u8 = 0x04;
const RTC_DAY:     u8 = 0x07;
const RTC_MONTH:   u8 = 0x08;
const RTC_YEAR:    u8 = 0x09;
const RTC_STAT_B:  u8 = 0x0B;

// Status register B bit 2 = 1 for binary, 0 for BCD
const RTC_BCD: u8 = 0x04;

#[derive(Debug, Clone, Copy)]
pub struct RtcDate {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
}

fn cmos_read(reg: u8) -> u8 {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") CMOS_IDX, in("al") reg, options(nomem, nostack));
        let val: u8;
        core::arch::asm!("in al, dx", out("al") val, in("dx") CMOS_DATA, options(nomem, nostack));
        val
    }
}

fn bcd_to_bin(bcd: u8) -> u8 {
    (bcd & 0x0F) + ((bcd >> 4) * 10)
}

/// Convert a CMOS year byte to a full year.
/// 0-79 → 2000-2079, 80-99 → 1980-1999.
fn century(year: u8) -> u16 {
    let y = year as u16;
    if y < 80 { y + 2000 } else { y + 1900 }
}

/// Read the current date and time from the CMOS RTC.
///
/// Uses the standard "read twice, verify seconds match" approach
/// to avoid reading across a tick boundary. Unlike the UIP-polling
/// method, this never blocks for up to a second.
/// Gives up after 100 retries and returns the last values read.
pub fn read_datetime() -> RtcDate {
    let is_bcd = cmos_read(RTC_STAT_B) & RTC_BCD == 0;

    for _ in 0..100 {
        let sec1 = cmos_read(RTC_SECONDS);
        let minutes = cmos_read(RTC_MINUTES);
        let hours   = cmos_read(RTC_HOURS);
        let day     = cmos_read(RTC_DAY);
        let month   = cmos_read(RTC_MONTH);
        let year    = cmos_read(RTC_YEAR);
        let sec2   = cmos_read(RTC_SECONDS);

        if sec1 == sec2 {
            if is_bcd {
                return RtcDate {
                    year: century(bcd_to_bin(year)),
                    month: bcd_to_bin(month),
                    day: bcd_to_bin(day),
                    hours: bcd_to_bin(hours),
                    minutes: bcd_to_bin(minutes),
                    seconds: bcd_to_bin(sec1),
                };
            } else {
                return RtcDate {
                    year: century(year),
                    month,
                    day,
                    hours,
                    minutes,
                    seconds: sec1,
                };
            }
        }
    }

    // After 100 retries, CMOS never settled. Return whatever we have.
    // Better than hanging the kernel.
    let sec = cmos_read(RTC_SECONDS);
    let minutes = cmos_read(RTC_MINUTES);
    let hours   = cmos_read(RTC_HOURS);
    let day     = cmos_read(RTC_DAY);
    let month   = cmos_read(RTC_MONTH);
    let year    = cmos_read(RTC_YEAR);
    if is_bcd {
        RtcDate {
            year: century(bcd_to_bin(year)),
            month: bcd_to_bin(month),
            day: bcd_to_bin(day),
            hours: bcd_to_bin(hours),
            minutes: bcd_to_bin(minutes),
            seconds: bcd_to_bin(sec),
        }
    } else {
        RtcDate {
            year: century(year),
            month,
            day,
            hours,
            minutes,
            seconds: sec,
        }
    }
}
