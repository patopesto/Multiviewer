#![allow(non_upper_case_globals)]

use serde::{Deserialize, Serialize};

// Four-character code helper matching C multi-character constant semantics.
// First character is the most significant byte.
pub const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    ((a as u32) << 24) | ((b as u32) << 16) | ((c as u32) << 8) | (d as u32)
}

// BMDVideoConnection bit flags
pub const bmdVideoConnectionUnspecified: u32 = 0;
pub const bmdVideoConnectionSDI: u32 = 1 << 0;
pub const bmdVideoConnectionHDMI: u32 = 1 << 1;
pub const bmdVideoConnectionOpticalSDI: u32 = 1 << 2;
pub const bmdVideoConnectionComponent: u32 = 1 << 3;
pub const bmdVideoConnectionComposite: u32 = 1 << 4;
pub const bmdVideoConnectionSVideo: u32 = 1 << 5;
pub const bmdVideoConnectionEthernet: u32 = 1 << 6;
pub const bmdVideoConnectionOpticalEthernet: u32 = 1 << 7;
pub const bmdVideoConnectionInternal: u32 = 1 << 8;

// BMDDisplayMode fourcc codes
pub const bmdModeNTSC: u32 = fourcc(b'n', b't', b's', b'c');
pub const bmdModeNTSC2398: u32 = fourcc(b'n', b't', b'2', b'3');
pub const bmdModePAL: u32 = fourcc(b'p', b'a', b'l', 0);
pub const bmdModeNTSCp: u32 = fourcc(b'n', b't', b's', b'p');
pub const bmdModePALp: u32 = fourcc(b'p', b'a', b'l', b'p');

pub const bmdModeHD1080p2398: u32 = fourcc(b'2', b'3', b'p', b's');
pub const bmdModeHD1080p24: u32 = fourcc(b'2', b'4', b'p', b's');
pub const bmdModeHD1080p25: u32 = fourcc(b'H', b'p', b'2', b'5');
pub const bmdModeHD1080p2997: u32 = fourcc(b'H', b'p', b'2', b'9');
pub const bmdModeHD1080p30: u32 = fourcc(b'H', b'p', b'3', b'0');
pub const bmdModeHD1080p4795: u32 = fourcc(b'H', b'p', b'4', b'7');
pub const bmdModeHD1080p48: u32 = fourcc(b'H', b'p', b'4', b'8');
pub const bmdModeHD1080p50: u32 = fourcc(b'H', b'p', b'5', b'0');
pub const bmdModeHD1080p5994: u32 = fourcc(b'H', b'p', b'5', b'9');
pub const bmdModeHD1080p6000: u32 = fourcc(b'H', b'p', b'6', b'0');
pub const bmdModeHD1080p9590: u32 = fourcc(b'H', b'p', b'9', b'5');
pub const bmdModeHD1080p96: u32 = fourcc(b'H', b'p', b'9', b'6');
pub const bmdModeHD1080p100: u32 = fourcc(b'H', b'p', b'1', b'0');
pub const bmdModeHD1080p11988: u32 = fourcc(b'H', b'p', b'1', b'1');
pub const bmdModeHD1080p120: u32 = fourcc(b'H', b'p', b'1', b'2');
pub const bmdModeHD1080i50: u32 = fourcc(b'H', b'i', b'5', b'0');
pub const bmdModeHD1080i5994: u32 = fourcc(b'H', b'i', b'5', b'9');
pub const bmdModeHD1080i6000: u32 = fourcc(b'H', b'i', b'6', b'0');

pub const bmdModeHD720p50: u32 = fourcc(b'h', b'p', b'5', b'0');
pub const bmdModeHD720p5994: u32 = fourcc(b'h', b'p', b'5', b'9');
pub const bmdModeHD720p60: u32 = fourcc(b'h', b'p', b'6', b'0');

pub const bmdMode2k2398: u32 = fourcc(b'2', b'k', b'2', b'3');
pub const bmdMode2k24: u32 = fourcc(b'2', b'k', b'2', b'4');
pub const bmdMode2k25: u32 = fourcc(b'2', b'k', b'2', b'5');

pub const bmdMode2kDCI2398: u32 = fourcc(b'2', b'd', b'2', b'3');
pub const bmdMode2kDCI24: u32 = fourcc(b'2', b'd', b'2', b'4');
pub const bmdMode2kDCI25: u32 = fourcc(b'2', b'd', b'2', b'5');
pub const bmdMode2kDCI2997: u32 = fourcc(b'2', b'd', b'2', b'9');
pub const bmdMode2kDCI30: u32 = fourcc(b'2', b'd', b'3', b'0');
pub const bmdMode2kDCI4795: u32 = fourcc(b'2', b'd', b'4', b'7');
pub const bmdMode2kDCI48: u32 = fourcc(b'2', b'd', b'4', b'8');
pub const bmdMode2kDCI50: u32 = fourcc(b'2', b'd', b'5', b'0');
pub const bmdMode2kDCI5994: u32 = fourcc(b'2', b'd', b'5', b'9');
pub const bmdMode2kDCI60: u32 = fourcc(b'2', b'd', b'6', b'0');
pub const bmdMode2kDCI9590: u32 = fourcc(b'2', b'd', b'9', b'5');
pub const bmdMode2kDCI96: u32 = fourcc(b'2', b'd', b'9', b'6');
pub const bmdMode2kDCI100: u32 = fourcc(b'2', b'd', b'1', b'0');
pub const bmdMode2kDCI11988: u32 = fourcc(b'2', b'd', b'1', b'1');
pub const bmdMode2kDCI120: u32 = fourcc(b'2', b'd', b'1', b'2');

pub const bmdMode4K2160p2398: u32 = fourcc(b'4', b'k', b'2', b'3');
pub const bmdMode4K2160p24: u32 = fourcc(b'4', b'k', b'2', b'4');
pub const bmdMode4K2160p25: u32 = fourcc(b'4', b'k', b'2', b'5');
pub const bmdMode4K2160p2997: u32 = fourcc(b'4', b'k', b'2', b'9');
pub const bmdMode4K2160p30: u32 = fourcc(b'4', b'k', b'3', b'0');
pub const bmdMode4K2160p4795: u32 = fourcc(b'4', b'k', b'4', b'7');
pub const bmdMode4K2160p48: u32 = fourcc(b'4', b'k', b'4', b'8');
pub const bmdMode4K2160p50: u32 = fourcc(b'4', b'k', b'5', b'0');
pub const bmdMode4K2160p5994: u32 = fourcc(b'4', b'k', b'5', b'9');
pub const bmdMode4K2160p60: u32 = fourcc(b'4', b'k', b'6', b'0');
pub const bmdMode4K2160p9590: u32 = fourcc(b'4', b'k', b'9', b'5');
pub const bmdMode4K2160p96: u32 = fourcc(b'4', b'k', b'9', b'6');
pub const bmdMode4K2160p100: u32 = fourcc(b'4', b'k', b'1', b'0');
pub const bmdMode4K2160p11988: u32 = fourcc(b'4', b'k', b'1', b'1');
pub const bmdMode4K2160p120: u32 = fourcc(b'4', b'k', b'1', b'2');

pub const bmdMode4kDCI2398: u32 = fourcc(b'4', b'd', b'2', b'3');
pub const bmdMode4kDCI24: u32 = fourcc(b'4', b'd', b'2', b'4');
pub const bmdMode4kDCI25: u32 = fourcc(b'4', b'd', b'2', b'5');
pub const bmdMode4kDCI2997: u32 = fourcc(b'4', b'd', b'2', b'9');
pub const bmdMode4kDCI30: u32 = fourcc(b'4', b'd', b'3', b'0');
pub const bmdMode4kDCI4795: u32 = fourcc(b'4', b'd', b'4', b'7');
pub const bmdMode4kDCI48: u32 = fourcc(b'4', b'd', b'4', b'8');
pub const bmdMode4kDCI50: u32 = fourcc(b'4', b'd', b'5', b'0');
pub const bmdMode4kDCI5994: u32 = fourcc(b'4', b'd', b'5', b'9');
pub const bmdMode4kDCI60: u32 = fourcc(b'4', b'd', b'6', b'0');
pub const bmdMode4kDCI9590: u32 = fourcc(b'4', b'd', b'9', b'5');
pub const bmdMode4kDCI96: u32 = fourcc(b'4', b'd', b'9', b'6');
pub const bmdMode4kDCI100: u32 = fourcc(b'4', b'd', b'1', b'0');
pub const bmdMode4kDCI11988: u32 = fourcc(b'4', b'd', b'1', b'1');
pub const bmdMode4kDCI120: u32 = fourcc(b'4', b'd', b'1', b'2');

pub const bmdMode8K4320p2398: u32 = fourcc(b'8', b'k', b'2', b'3');
pub const bmdMode8K4320p24: u32 = fourcc(b'8', b'k', b'2', b'4');
pub const bmdMode8K4320p25: u32 = fourcc(b'8', b'k', b'2', b'5');
pub const bmdMode8K4320p2997: u32 = fourcc(b'8', b'k', b'2', b'9');
pub const bmdMode8K4320p30: u32 = fourcc(b'8', b'k', b'3', b'0');
pub const bmdMode8K4320p4795: u32 = fourcc(b'8', b'k', b'4', b'7');
pub const bmdMode8K4320p48: u32 = fourcc(b'8', b'k', b'4', b'8');
pub const bmdMode8K4320p50: u32 = fourcc(b'8', b'k', b'5', b'0');
pub const bmdMode8K4320p5994: u32 = fourcc(b'8', b'k', b'5', b'9');
pub const bmdMode8K4320p60: u32 = fourcc(b'8', b'k', b'6', b'0');

pub const bmdMode8kDCI2398: u32 = fourcc(b'8', b'd', b'2', b'3');
pub const bmdMode8kDCI24: u32 = fourcc(b'8', b'd', b'2', b'4');
pub const bmdMode8kDCI25: u32 = fourcc(b'8', b'd', b'2', b'5');
pub const bmdMode8kDCI2997: u32 = fourcc(b'8', b'd', b'2', b'9');
pub const bmdMode8kDCI30: u32 = fourcc(b'8', b'd', b'3', b'0');
pub const bmdMode8kDCI4795: u32 = fourcc(b'8', b'd', b'4', b'7');
pub const bmdMode8kDCI48: u32 = fourcc(b'8', b'd', b'4', b'8');
pub const bmdMode8kDCI50: u32 = fourcc(b'8', b'd', b'5', b'0');
pub const bmdMode8kDCI5994: u32 = fourcc(b'8', b'd', b'5', b'9');
pub const bmdMode8kDCI60: u32 = fourcc(b'8', b'd', b'6', b'0');

pub const bmdMode640x480p60: u32 = fourcc(b'v', b'g', b'a', b'6');
pub const bmdMode800x600p60: u32 = fourcc(b's', b'v', b'g', b'6');
pub const bmdMode1440x900p50: u32 = fourcc(b'w', b'x', b'g', b'5');
pub const bmdMode1440x900p60: u32 = fourcc(b'w', b'x', b'g', b'6');
pub const bmdMode1440x1080p50: u32 = fourcc(b's', b'x', b'g', b'5');
pub const bmdMode1440x1080p60: u32 = fourcc(b's', b'x', b'g', b'6');
pub const bmdMode1600x1200p50: u32 = fourcc(b'u', b'x', b'g', b'5');
pub const bmdMode1600x1200p60: u32 = fourcc(b'u', b'x', b'g', b'6');
pub const bmdMode1920x1200p50: u32 = fourcc(b'w', b'u', b'x', b'5');
pub const bmdMode1920x1200p60: u32 = fourcc(b'w', b'u', b'x', b'6');
pub const bmdMode1920x1440p50: u32 = fourcc(b'1', b'9', b'4', b'5');
pub const bmdMode1920x1440p60: u32 = fourcc(b'1', b'9', b'4', b'6');
pub const bmdMode2560x1440p50: u32 = fourcc(b'w', b'q', b'h', b'5');
pub const bmdMode2560x1440p60: u32 = fourcc(b'w', b'q', b'h', b'6');
pub const bmdMode2560x1600p50: u32 = fourcc(b'w', b'q', b'x', b'5');
pub const bmdMode2560x1600p60: u32 = fourcc(b'w', b'q', b'x', b'6');
pub const bmdModeUnknown: u32 = fourcc(b'i', b'u', b'n', b'k');

// BMDPixelFormat constants
pub const bmdFormatUnspecified: u32 = 0;
pub const bmdFormat8BitYUV: u32 = fourcc(b'2', b'v', b'u', b'y');
pub const bmdFormat10BitYUV: u32 = fourcc(b'v', b'2', b'1', b'0');
pub const bmdFormat10BitYUVA: u32 = fourcc(b'A', b'y', b'1', b'0');
pub const bmdFormat8BitARGB: u32 = 32;
pub const bmdFormat8BitBGRA: u32 = fourcc(b'B', b'G', b'R', b'A');
pub const bmdFormat10BitRGB: u32 = fourcc(b'r', b'2', b'1', b'0');
pub const bmdFormat12BitRGB: u32 = fourcc(b'R', b'1', b'2', b'B');
pub const bmdFormat12BitRGBLE: u32 = fourcc(b'R', b'1', b'2', b'L');
pub const bmdFormat10BitRGBXLE: u32 = fourcc(b'R', b'1', b'0', b'l');
pub const bmdFormat10BitRGBX: u32 = fourcc(b'R', b'1', b'0', b'b');
pub const bmdFormatH265: u32 = fourcc(b'h', b'e', b'v', b'1');
pub const bmdFormatDNxHR: u32 = fourcc(b'A', b'V', b'd', b'h');

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u32)]
pub enum VideoConnection {
    #[default]
    Unspecified = bmdVideoConnectionUnspecified,
    Sdi = bmdVideoConnectionSDI,
    Hdmi = bmdVideoConnectionHDMI,
    OpticalSdi = bmdVideoConnectionOpticalSDI,
    Component = bmdVideoConnectionComponent,
    Composite = bmdVideoConnectionComposite,
    SVideo = bmdVideoConnectionSVideo,
    Ethernet = bmdVideoConnectionEthernet,
    OpticalEthernet = bmdVideoConnectionOpticalEthernet,
    Internal = bmdVideoConnectionInternal,
}

impl VideoConnection {
    pub const ALL: [VideoConnection; 9] = [
        VideoConnection::Sdi,
        VideoConnection::Hdmi,
        VideoConnection::OpticalSdi,
        VideoConnection::Component,
        VideoConnection::Composite,
        VideoConnection::SVideo,
        VideoConnection::Ethernet,
        VideoConnection::OpticalEthernet,
        VideoConnection::Internal,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            VideoConnection::Unspecified => "Unspecified",
            VideoConnection::Sdi => "SDI",
            VideoConnection::Hdmi => "HDMI",
            VideoConnection::OpticalSdi => "Optical SDI",
            VideoConnection::Component => "Component",
            VideoConnection::Composite => "Composite",
            VideoConnection::SVideo => "S-Video",
            VideoConnection::Ethernet => "Ethernet",
            VideoConnection::OpticalEthernet => "Optical Ethernet",
            VideoConnection::Internal => "Internal",
        }
    }
}

impl From<VideoConnection> for u32 {
    fn from(conn: VideoConnection) -> Self {
        conn as u32
    }
}

impl TryFrom<u32> for VideoConnection {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            v if v == bmdVideoConnectionUnspecified => Ok(VideoConnection::Unspecified),
            v if v == bmdVideoConnectionSDI => Ok(VideoConnection::Sdi),
            v if v == bmdVideoConnectionHDMI => Ok(VideoConnection::Hdmi),
            v if v == bmdVideoConnectionOpticalSDI => Ok(VideoConnection::OpticalSdi),
            v if v == bmdVideoConnectionComponent => Ok(VideoConnection::Component),
            v if v == bmdVideoConnectionComposite => Ok(VideoConnection::Composite),
            v if v == bmdVideoConnectionSVideo => Ok(VideoConnection::SVideo),
            v if v == bmdVideoConnectionEthernet => Ok(VideoConnection::Ethernet),
            v if v == bmdVideoConnectionOpticalEthernet => Ok(VideoConnection::OpticalEthernet),
            v if v == bmdVideoConnectionInternal => Ok(VideoConnection::Internal),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VideoConnections(pub u32);

impl VideoConnections {
    pub const EMPTY: Self = Self(0);

    pub fn contains(&self, conn: VideoConnection) -> bool {
        self.0 & (conn as u32) != 0
    }

    pub fn iter(&self) -> impl Iterator<Item = VideoConnection> {
        VideoConnection::ALL.into_iter().filter(|c| self.contains(*c))
    }

    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    pub fn default_connection(&self) -> VideoConnection {
        for &conn in &VideoConnection::ALL {
            if self.contains(conn) {
                return conn;
            }
        }
        VideoConnection::Sdi
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum DecklinkPixelFormat {
    Unspecified = 0,
    Uyvy422 = bmdFormat8BitYUV,
    Yuv10Bit = bmdFormat10BitYUV,
    Argb8 = bmdFormat8BitARGB,
    Bgra8 = bmdFormat8BitBGRA,
    Rgb10Bit = bmdFormat10BitRGB,
    Rgb12Bit = bmdFormat12BitRGB,
    Rgb12BitLe = bmdFormat12BitRGBLE,
    H265 = bmdFormatH265,
    DnxHr = bmdFormatDNxHR,
}

impl From<DecklinkPixelFormat> for u32 {
    fn from(fmt: DecklinkPixelFormat) -> Self {
        fmt as u32
    }
}

impl TryFrom<u32> for DecklinkPixelFormat {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(DecklinkPixelFormat::Unspecified),
            v if v == bmdFormat8BitYUV => Ok(DecklinkPixelFormat::Uyvy422),
            v if v == bmdFormat10BitYUV => Ok(DecklinkPixelFormat::Yuv10Bit),
            v if v == bmdFormat8BitARGB => Ok(DecklinkPixelFormat::Argb8),
            v if v == bmdFormat8BitBGRA => Ok(DecklinkPixelFormat::Bgra8),
            v if v == bmdFormat10BitRGB => Ok(DecklinkPixelFormat::Rgb10Bit),
            v if v == bmdFormat12BitRGB => Ok(DecklinkPixelFormat::Rgb12Bit),
            v if v == bmdFormat12BitRGBLE => Ok(DecklinkPixelFormat::Rgb12BitLe),
            v if v == bmdFormatH265 => Ok(DecklinkPixelFormat::H265),
            v if v == bmdFormatDNxHR => Ok(DecklinkPixelFormat::DnxHr),
            _ => Err(()),
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum DisplayMode {
    // SD Modes
    Ntsc = bmdModeNTSC,
    Ntsc2398 = bmdModeNTSC2398,
    Pal = bmdModePAL,
    NtscP = bmdModeNTSCp,
    PalP = bmdModePALp,

    // HD 1080 Modes
    Hd1080p2398 = bmdModeHD1080p2398,
    Hd1080p24 = bmdModeHD1080p24,
    Hd1080p25 = bmdModeHD1080p25,
    Hd1080p2997 = bmdModeHD1080p2997,
    Hd1080p30 = bmdModeHD1080p30,
    Hd1080p48 = bmdModeHD1080p48,
    Hd1080p50 = bmdModeHD1080p50,
    Hd1080p5994 = bmdModeHD1080p5994,
    #[default]
    Hd1080p6000 = bmdModeHD1080p6000,
    Hd1080p9590 = bmdModeHD1080p9590,
    Hd1080p96 = bmdModeHD1080p96,
    Hd1080p100 = bmdModeHD1080p100,
    Hd1080p11988 = bmdModeHD1080p11988,
    Hd1080p120 = bmdModeHD1080p120,
    Hd1080i50 = bmdModeHD1080i50,
    Hd1080i5994 = bmdModeHD1080i5994,
    Hd1080i6000 = bmdModeHD1080i6000,

    // HD 720 Modes
    Hd720p50 = bmdModeHD720p50,
    Hd720p5994 = bmdModeHD720p5994,
    Hd720p60 = bmdModeHD720p60,

    // 2K Modes
    Mode2k2398 = bmdMode2k2398,
    Mode2k24 = bmdMode2k24,
    Mode2k25 = bmdMode2k25,

    // 2K DCI Modes
    Mode2kDCI2398 = bmdMode2kDCI2398,
    Mode2kDCI24 = bmdMode2kDCI24,
    Mode2kDCI25 = bmdMode2kDCI25,
    Mode2kDCI2997 = bmdMode2kDCI2997,
    Mode2kDCI30 = bmdMode2kDCI30,
    Mode2kDCI48 = bmdMode2kDCI48,
    Mode2kDCI50 = bmdMode2kDCI50,
    Mode2kDCI5994 = bmdMode2kDCI5994,
    Mode2kDCI60 = bmdMode2kDCI60,
    Mode2kDCI9590 = bmdMode2kDCI9590,
    Mode2kDCI96 = bmdMode2kDCI96,
    Mode2kDCI100 = bmdMode2kDCI100,
    Mode2kDCI11988 = bmdMode2kDCI11988,
    Mode2kDCI120 = bmdMode2kDCI120,

    // 4K UHD Modes
    Mode4K2160p2398 = bmdMode4K2160p2398,
    Mode4K2160p24 = bmdMode4K2160p24,
    Mode4K2160p25 = bmdMode4K2160p25,
    Mode4K2160p2997 = bmdMode4K2160p2997,
    Mode4K2160p30 = bmdMode4K2160p30,
    Mode4K2160p48 = bmdMode4K2160p48,
    Mode4K2160p50 = bmdMode4K2160p50,
    Mode4K2160p5994 = bmdMode4K2160p5994,
    Mode4K2160p60 = bmdMode4K2160p60,
    Mode4K2160p9590 = bmdMode4K2160p9590,
    Mode4K2160p96 = bmdMode4K2160p96,
    Mode4K2160p100 = bmdMode4K2160p100,
    Mode4K2160p11988 = bmdMode4K2160p11988,
    Mode4K2160p120 = bmdMode4K2160p120,

    // 4K DCI Modes
    Mode4kDCI2398 = bmdMode4kDCI2398,
    Mode4kDCI24 = bmdMode4kDCI24,
    Mode4kDCI25 = bmdMode4kDCI25,
    Mode4kDCI2997 = bmdMode4kDCI2997,
    Mode4kDCI30 = bmdMode4kDCI30,
    Mode4kDCI48 = bmdMode4kDCI48,
    Mode4kDCI50 = bmdMode4kDCI50,
    Mode4kDCI5994 = bmdMode4kDCI5994,
    Mode4kDCI60 = bmdMode4kDCI60,
    Mode4kDCI9590 = bmdMode4kDCI9590,
    Mode4kDCI96 = bmdMode4kDCI96,
    Mode4kDCI100 = bmdMode4kDCI100,
    Mode4kDCI11988 = bmdMode4kDCI11988,
    Mode4kDCI120 = bmdMode4kDCI120,

    // 8K UHD Modes
    Mode8K4320p2398 = bmdMode8K4320p2398,
    Mode8K4320p24 = bmdMode8K4320p24,
    Mode8K4320p25 = bmdMode8K4320p25,
    Mode8K4320p2997 = bmdMode8K4320p2997,
    Mode8K4320p30 = bmdMode8K4320p30,
    Mode8K4320p48 = bmdMode8K4320p48,
    Mode8K4320p50 = bmdMode8K4320p50,
    Mode8K4320p5994 = bmdMode8K4320p5994,
    Mode8K4320p60 = bmdMode8K4320p60,

    // 8K DCI Modes
    Mode8kDCI2398 = bmdMode8kDCI2398,
    Mode8kDCI24 = bmdMode8kDCI24,
    Mode8kDCI25 = bmdMode8kDCI25,
    Mode8kDCI2997 = bmdMode8kDCI2997,
    Mode8kDCI30 = bmdMode8kDCI30,
    Mode8kDCI48 = bmdMode8kDCI48,
    Mode8kDCI50 = bmdMode8kDCI50,
    Mode8kDCI5994 = bmdMode8kDCI5994,
    Mode8kDCI60 = bmdMode8kDCI60,

    // PC Modes
    Mode640x480p60 = bmdMode640x480p60,
    Mode800x600p60 = bmdMode800x600p60,
    Mode1440x900p50 = bmdMode1440x900p50,
    Mode1440x900p60 = bmdMode1440x900p60,
    Mode1440x1080p50 = bmdMode1440x1080p50,
    Mode1440x1080p60 = bmdMode1440x1080p60,
    Mode1600x1200p50 = bmdMode1600x1200p50,
    Mode1600x1200p60 = bmdMode1600x1200p60,
    Mode1920x1200p50 = bmdMode1920x1200p50,
    Mode1920x1200p60 = bmdMode1920x1200p60,
    Mode1920x1440p50 = bmdMode1920x1440p50,
    Mode1920x1440p60 = bmdMode1920x1440p60,
    Mode2560x1440p50 = bmdMode2560x1440p50,
    Mode2560x1440p60 = bmdMode2560x1440p60,
    Mode2560x1600p50 = bmdMode2560x1600p50,
    Mode2560x1600p60 = bmdMode2560x1600p60,

    Unknown = 0x69756E6B,
}

impl From<DisplayMode> for u32 {
    fn from(mode: DisplayMode) -> Self {
        mode as u32
    }
}

impl serde::Serialize for DisplayMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(*self as u32)
    }
}

impl<'de> serde::Deserialize<'de> for DisplayMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u32::deserialize(deserializer)?;
        DisplayMode::try_from(value)
            .map_err(|_| serde::de::Error::custom(format!("unknown display mode {value}")))
    }
}

impl TryFrom<u32> for DisplayMode {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            v if v == bmdModeNTSC => Ok(DisplayMode::Ntsc),
            v if v == bmdModeNTSC2398 => Ok(DisplayMode::Ntsc2398),
            v if v == bmdModePAL => Ok(DisplayMode::Pal),
            v if v == bmdModeNTSCp => Ok(DisplayMode::NtscP),
            v if v == bmdModePALp => Ok(DisplayMode::PalP),

            v if v == bmdModeHD1080p2398 => Ok(DisplayMode::Hd1080p2398),
            v if v == bmdModeHD1080p24 => Ok(DisplayMode::Hd1080p24),
            v if v == bmdModeHD1080p25 => Ok(DisplayMode::Hd1080p25),
            v if v == bmdModeHD1080p2997 => Ok(DisplayMode::Hd1080p2997),
            v if v == bmdModeHD1080p30 => Ok(DisplayMode::Hd1080p30),
            v if v == bmdModeHD1080p48 => Ok(DisplayMode::Hd1080p48),
            v if v == bmdModeHD1080p50 => Ok(DisplayMode::Hd1080p50),
            v if v == bmdModeHD1080p5994 => Ok(DisplayMode::Hd1080p5994),
            v if v == bmdModeHD1080p6000 => Ok(DisplayMode::Hd1080p6000),
            v if v == bmdModeHD1080p9590 => Ok(DisplayMode::Hd1080p9590),
            v if v == bmdModeHD1080p96 => Ok(DisplayMode::Hd1080p96),
            v if v == bmdModeHD1080p100 => Ok(DisplayMode::Hd1080p100),
            v if v == bmdModeHD1080p11988 => Ok(DisplayMode::Hd1080p11988),
            v if v == bmdModeHD1080p120 => Ok(DisplayMode::Hd1080p120),
            v if v == bmdModeHD1080i50 => Ok(DisplayMode::Hd1080i50),
            v if v == bmdModeHD1080i5994 => Ok(DisplayMode::Hd1080i5994),
            v if v == bmdModeHD1080i6000 => Ok(DisplayMode::Hd1080i6000),

            v if v == bmdModeHD720p50 => Ok(DisplayMode::Hd720p50),
            v if v == bmdModeHD720p5994 => Ok(DisplayMode::Hd720p5994),
            v if v == bmdModeHD720p60 => Ok(DisplayMode::Hd720p60),

            v if v == bmdMode2k2398 => Ok(DisplayMode::Mode2k2398),
            v if v == bmdMode2k24 => Ok(DisplayMode::Mode2k24),
            v if v == bmdMode2k25 => Ok(DisplayMode::Mode2k25),

            v if v == bmdMode2kDCI2398 => Ok(DisplayMode::Mode2kDCI2398),
            v if v == bmdMode2kDCI24 => Ok(DisplayMode::Mode2kDCI24),
            v if v == bmdMode2kDCI25 => Ok(DisplayMode::Mode2kDCI25),
            v if v == bmdMode2kDCI2997 => Ok(DisplayMode::Mode2kDCI2997),
            v if v == bmdMode2kDCI30 => Ok(DisplayMode::Mode2kDCI30),
            v if v == bmdMode2kDCI48 => Ok(DisplayMode::Mode2kDCI48),
            v if v == bmdMode2kDCI50 => Ok(DisplayMode::Mode2kDCI50),
            v if v == bmdMode2kDCI5994 => Ok(DisplayMode::Mode2kDCI5994),
            v if v == bmdMode2kDCI60 => Ok(DisplayMode::Mode2kDCI60),
            v if v == bmdMode2kDCI9590 => Ok(DisplayMode::Mode2kDCI9590),
            v if v == bmdMode2kDCI96 => Ok(DisplayMode::Mode2kDCI96),
            v if v == bmdMode2kDCI100 => Ok(DisplayMode::Mode2kDCI100),
            v if v == bmdMode2kDCI11988 => Ok(DisplayMode::Mode2kDCI11988),
            v if v == bmdMode2kDCI120 => Ok(DisplayMode::Mode2kDCI120),

            v if v == bmdMode4K2160p2398 => Ok(DisplayMode::Mode4K2160p2398),
            v if v == bmdMode4K2160p24 => Ok(DisplayMode::Mode4K2160p24),
            v if v == bmdMode4K2160p25 => Ok(DisplayMode::Mode4K2160p25),
            v if v == bmdMode4K2160p2997 => Ok(DisplayMode::Mode4K2160p2997),
            v if v == bmdMode4K2160p30 => Ok(DisplayMode::Mode4K2160p30),
            v if v == bmdMode4K2160p48 => Ok(DisplayMode::Mode4K2160p48),
            v if v == bmdMode4K2160p50 => Ok(DisplayMode::Mode4K2160p50),
            v if v == bmdMode4K2160p5994 => Ok(DisplayMode::Mode4K2160p5994),
            v if v == bmdMode4K2160p60 => Ok(DisplayMode::Mode4K2160p60),
            v if v == bmdMode4K2160p9590 => Ok(DisplayMode::Mode4K2160p9590),
            v if v == bmdMode4K2160p96 => Ok(DisplayMode::Mode4K2160p96),
            v if v == bmdMode4K2160p100 => Ok(DisplayMode::Mode4K2160p100),
            v if v == bmdMode4K2160p11988 => Ok(DisplayMode::Mode4K2160p11988),
            v if v == bmdMode4K2160p120 => Ok(DisplayMode::Mode4K2160p120),

            v if v == bmdMode4kDCI2398 => Ok(DisplayMode::Mode4kDCI2398),
            v if v == bmdMode4kDCI24 => Ok(DisplayMode::Mode4kDCI24),
            v if v == bmdMode4kDCI25 => Ok(DisplayMode::Mode4kDCI25),
            v if v == bmdMode4kDCI2997 => Ok(DisplayMode::Mode4kDCI2997),
            v if v == bmdMode4kDCI30 => Ok(DisplayMode::Mode4kDCI30),
            v if v == bmdMode4kDCI48 => Ok(DisplayMode::Mode4kDCI48),
            v if v == bmdMode4kDCI50 => Ok(DisplayMode::Mode4kDCI50),
            v if v == bmdMode4kDCI5994 => Ok(DisplayMode::Mode4kDCI5994),
            v if v == bmdMode4kDCI60 => Ok(DisplayMode::Mode4kDCI60),
            v if v == bmdMode4kDCI9590 => Ok(DisplayMode::Mode4kDCI9590),
            v if v == bmdMode4kDCI96 => Ok(DisplayMode::Mode4kDCI96),
            v if v == bmdMode4kDCI100 => Ok(DisplayMode::Mode4kDCI100),
            v if v == bmdMode4kDCI11988 => Ok(DisplayMode::Mode4kDCI11988),
            v if v == bmdMode4kDCI120 => Ok(DisplayMode::Mode4kDCI120),

            v if v == bmdMode8K4320p2398 => Ok(DisplayMode::Mode8K4320p2398),
            v if v == bmdMode8K4320p24 => Ok(DisplayMode::Mode8K4320p24),
            v if v == bmdMode8K4320p25 => Ok(DisplayMode::Mode8K4320p25),
            v if v == bmdMode8K4320p2997 => Ok(DisplayMode::Mode8K4320p2997),
            v if v == bmdMode8K4320p30 => Ok(DisplayMode::Mode8K4320p30),
            v if v == bmdMode8K4320p48 => Ok(DisplayMode::Mode8K4320p48),
            v if v == bmdMode8K4320p50 => Ok(DisplayMode::Mode8K4320p50),
            v if v == bmdMode8K4320p5994 => Ok(DisplayMode::Mode8K4320p5994),
            v if v == bmdMode8K4320p60 => Ok(DisplayMode::Mode8K4320p60),

            v if v == bmdMode8kDCI2398 => Ok(DisplayMode::Mode8kDCI2398),
            v if v == bmdMode8kDCI24 => Ok(DisplayMode::Mode8kDCI24),
            v if v == bmdMode8kDCI25 => Ok(DisplayMode::Mode8kDCI25),
            v if v == bmdMode8kDCI2997 => Ok(DisplayMode::Mode8kDCI2997),
            v if v == bmdMode8kDCI30 => Ok(DisplayMode::Mode8kDCI30),
            v if v == bmdMode8kDCI48 => Ok(DisplayMode::Mode8kDCI48),
            v if v == bmdMode8kDCI50 => Ok(DisplayMode::Mode8kDCI50),
            v if v == bmdMode8kDCI5994 => Ok(DisplayMode::Mode8kDCI5994),
            v if v == bmdMode8kDCI60 => Ok(DisplayMode::Mode8kDCI60),

            v if v == bmdMode640x480p60 => Ok(DisplayMode::Mode640x480p60),
            v if v == bmdMode800x600p60 => Ok(DisplayMode::Mode800x600p60),
            v if v == bmdMode1440x900p50 => Ok(DisplayMode::Mode1440x900p50),
            v if v == bmdMode1440x900p60 => Ok(DisplayMode::Mode1440x900p60),
            v if v == bmdMode1440x1080p50 => Ok(DisplayMode::Mode1440x1080p50),
            v if v == bmdMode1440x1080p60 => Ok(DisplayMode::Mode1440x1080p60),
            v if v == bmdMode1600x1200p50 => Ok(DisplayMode::Mode1600x1200p50),
            v if v == bmdMode1600x1200p60 => Ok(DisplayMode::Mode1600x1200p60),
            v if v == bmdMode1920x1200p50 => Ok(DisplayMode::Mode1920x1200p50),
            v if v == bmdMode1920x1200p60 => Ok(DisplayMode::Mode1920x1200p60),
            v if v == bmdMode1920x1440p50 => Ok(DisplayMode::Mode1920x1440p50),
            v if v == bmdMode1920x1440p60 => Ok(DisplayMode::Mode1920x1440p60),
            v if v == bmdMode2560x1440p50 => Ok(DisplayMode::Mode2560x1440p50),
            v if v == bmdMode2560x1440p60 => Ok(DisplayMode::Mode2560x1440p60),
            v if v == bmdMode2560x1600p50 => Ok(DisplayMode::Mode2560x1600p50),
            v if v == bmdMode2560x1600p60 => Ok(DisplayMode::Mode2560x1600p60),
            _ => Err(()),
        }
    }
}
