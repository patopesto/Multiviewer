#ifndef DECKLINK_SDK_ENUMS_H
#define DECKLINK_SDK_ENUMS_H

#include <stdint.h>

// BMDVideoConnection values from DeckLinkAPITypes.h
typedef uint32_t BMDVideoConnection;
enum {
    bmdVideoConnectionUnspecified = 0,
    bmdVideoConnectionSDI = 1 << 0,
    bmdVideoConnectionHDMI = 1 << 1,
    bmdVideoConnectionOpticalSDI = 1 << 2,
    bmdVideoConnectionComponent = 1 << 3,
    bmdVideoConnectionComposite = 1 << 4,
    bmdVideoConnectionSVideo = 1 << 5,
    bmdVideoConnectionEthernet = 1 << 6,
    bmdVideoConnectionOpticalEthernet = 1 << 7,
    bmdVideoConnectionInternal = 1 << 8,
};

// BMDDisplayMode values from DeckLinkAPIModes.h
typedef uint32_t BMDDisplayMode;
enum {
    /* SD Modes */

    bmdModeNTSC = 'ntsc',
    bmdModeNTSC2398 = 'nt23',
    bmdModePAL  = 'pal',
    bmdModeNTSCp = 'ntsp',
    bmdModePALp = 'palp',

    /* HD 1080 Modes */

    bmdModeHD1080p2398 = '23ps',
    bmdModeHD1080p24 = '24ps',
    bmdModeHD1080p25 = 'Hp25',
    bmdModeHD1080p2997 = 'Hp29',
    bmdModeHD1080p30 = 'Hp30',
    bmdModeHD1080p4795 = 'Hp47',
    bmdModeHD1080p48 = 'Hp48',
    bmdModeHD1080p50 = 'Hp50',
    bmdModeHD1080p5994 = 'Hp59',
    bmdModeHD1080p6000 = 'Hp60',
    bmdModeHD1080p9590 = 'Hp95',
    bmdModeHD1080p96 = 'Hp96',
    bmdModeHD1080p100 = 'Hp10',
    bmdModeHD1080p11988 = 'Hp11',
    bmdModeHD1080p120 = 'Hp12',
    bmdModeHD1080i50 = 'Hi50',
    bmdModeHD1080i5994 = 'Hi59',
    bmdModeHD1080i6000 = 'Hi60',

    /* HD 720 Modes */

    bmdModeHD720p50 = 'hp50',
    bmdModeHD720p5994 = 'hp59',
    bmdModeHD720p60 = 'hp60',

    /* 2K Modes */

    bmdMode2k2398 = '2k23',
    bmdMode2k24 = '2k24',
    bmdMode2k25 = '2k25',

    /* 2K DCI Modes */

    bmdMode2kDCI2398 = '2d23',
    bmdMode2kDCI24 = '2d24',
    bmdMode2kDCI25 = '2d25',
    bmdMode2kDCI2997 = '2d29',
    bmdMode2kDCI30 = '2d30',
    bmdMode2kDCI4795 = '2d47',
    bmdMode2kDCI48 = '2d48',
    bmdMode2kDCI50 = '2d50',
    bmdMode2kDCI5994 = '2d59',
    bmdMode2kDCI60 = '2d60',
    bmdMode2kDCI9590 = '2d95',
    bmdMode2kDCI96 = '2d96',
    bmdMode2kDCI100 = '2d10',
    bmdMode2kDCI11988 = '2d11',
    bmdMode2kDCI120 = '2d12',

    /* 4K UHD Modes */

    bmdMode4K2160p2398 = '4k23',
    bmdMode4K2160p24 = '4k24',
    bmdMode4K2160p25 = '4k25',
    bmdMode4K2160p2997 = '4k29',
    bmdMode4K2160p30 = '4k30',
    bmdMode4K2160p4795 = '4k47',
    bmdMode4K2160p48 = '4k48',
    bmdMode4K2160p50 = '4k50',
    bmdMode4K2160p5994 = '4k59',
    bmdMode4K2160p60 = '4k60',
    bmdMode4K2160p9590 = '4k95',
    bmdMode4K2160p96 = '4k96',
    bmdMode4K2160p100 = '4k10',
    bmdMode4K2160p11988 = '4k11',
    bmdMode4K2160p120 = '4k12',

    /* 4K DCI Modes */

    bmdMode4kDCI2398 = '4d23',
    bmdMode4kDCI24 = '4d24',
    bmdMode4kDCI25 = '4d25',
    bmdMode4kDCI2997 = '4d29',
    bmdMode4kDCI30 = '4d30',
    bmdMode4kDCI4795 = '4d47',
    bmdMode4kDCI48 = '4d48',
    bmdMode4kDCI50 = '4d50',
    bmdMode4kDCI5994 = '4d59',
    bmdMode4kDCI60 = '4d60',
    bmdMode4kDCI9590 = '4d95',
    bmdMode4kDCI96 = '4d96',
    bmdMode4kDCI100 = '4d10',
    bmdMode4kDCI11988 = '4d11',
    bmdMode4kDCI120 = '4d12',

    /* 8K UHD Modes */

    bmdMode8K4320p2398 = '8k23',
    bmdMode8K4320p24 = '8k24',
    bmdMode8K4320p25 = '8k25',
    bmdMode8K4320p2997 = '8k29',
    bmdMode8K4320p30 = '8k30',
    bmdMode8K4320p4795 = '8k47',
    bmdMode8K4320p48 = '8k48',
    bmdMode8K4320p50 = '8k50',
    bmdMode8K4320p5994 = '8k59',
    bmdMode8K4320p60 = '8k60',

    /* 8K DCI Modes */

    bmdMode8kDCI2398 = '8d23',
    bmdMode8kDCI24 = '8d24',
    bmdMode8kDCI25 = '8d25',
    bmdMode8kDCI2997 = '8d29',
    bmdMode8kDCI30 = '8d30',
    bmdMode8kDCI4795 = '8d47',
    bmdMode8kDCI48 = '8d48',
    bmdMode8kDCI50 = '8d50',
    bmdMode8kDCI5994 = '8d59',
    bmdMode8kDCI60 = '8d60',

    /* PC Modes */

    bmdMode640x480p60 = 'vga6',
    bmdMode800x600p60 = 'svg6',
    bmdMode1440x900p50 = 'wxg5',
    bmdMode1440x900p60 = 'wxg6',
    bmdMode1440x1080p50 = 'sxg5',
    bmdMode1440x1080p60 = 'sxg6',
    bmdMode1600x1200p50 = 'uxg5',
    bmdMode1600x1200p60 = 'uxg6',
    bmdMode1920x1200p50 = 'wux5',
    bmdMode1920x1200p60 = 'wux6',
    bmdMode1920x1440p50 = '1945',
    bmdMode1920x1440p60 = '1946',
    bmdMode2560x1440p50 = 'wqh5',
    bmdMode2560x1440p60 = 'wqh6',
    bmdMode2560x1600p50 = 'wqx5',
    bmdMode2560x1600p60 = 'wqx6',
    bmdModeUnknown = 'iunk',
};

// BMDPixelFormat values from DeckLinkAPIModes.h
typedef uint32_t BMDPixelFormat;
enum {
    bmdFormatUnspecified = 0,
    bmdFormat8BitYUV = '2vuy',
    bmdFormat10BitYUV = 'v210',
    bmdFormat10BitYUVA = 'Ay10',
    bmdFormat8BitARGB = 32,
    bmdFormat8BitBGRA = 'BGRA',
    bmdFormat10BitRGB = 'r210',
    bmdFormat12BitRGB = 'R12B',
    bmdFormat12BitRGBLE = 'R12L',
    bmdFormat10BitRGBXLE = 'R10l',
    bmdFormat10BitRGBX = 'R10b',

    bmdFormatH265 = 'hev1',
    bmdFormatDNxHR = 'AVdh',
};

#endif
