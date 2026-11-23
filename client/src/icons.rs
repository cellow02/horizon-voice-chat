//! The mod's own pixel-art icons, 16 by 16, drawn with filled rectangles: a native mod cannot
//! bring textures of its own into the GUI atlas. Drawn for this mod; none is taken from
//! another mod.

use mod_api::DrawContext;

/// One icon: 16 rows of 16 palette characters (see [`color`]).
pub type Icon = [&'static str; 16];

/// The colour of a palette character; `.` is transparent.
fn color(c: u8, white: u32) -> Option<u32> {
    Some(match c {
        b'k' => 0xff20_2020,
        b'w' => white,
        b'l' => 0xffbe_bebe,
        b'g' => 0xff6e_6e6e,
        b'd' => 0xff46_4646,
        b'r' => 0xffe5_2b2b,
        b'G' => 0xff3c_cb3c,
        b'y' => 0xfff0_be32,
        b's' => 0xffe0_a578,
        b'b' => 0xff32_64d2,
        b'h' => 0xff5a_3a1a,
        b'c' => 0xff28_a0c8,
        _ => return None,
    })
}

/// Draws `icon` with its top left at `at`, `size` GUI pixels square, its white parts in
/// `white`.
pub fn draw_tinted(draw: &mut dyn DrawContext, icon: &Icon, at: [f32; 2], size: f32, white: u32) {
    let pixel = size / 16.0;
    for (y, row) in icon.iter().enumerate() {
        let row = row.as_bytes();
        let mut x = 0;
        while x < row.len() {
            let Some(argb) = color(row[x], white) else {
                x += 1;
                continue;
            };
            // One rectangle for a run of the same colour.
            let start = x;
            while x < row.len() && row[x] == row[start] {
                x += 1;
            }
            draw.fill(
                [
                    at[0] + start as f32 * pixel,
                    at[1] + y as f32 * pixel,
                    (x - start) as f32 * pixel,
                    pixel,
                ],
                argb,
            );
        }
    }
}

pub fn draw(draw: &mut dyn DrawContext, icon: &Icon, at: [f32; 2], size: f32) {
    draw_tinted(draw, icon, at, size, 0xffff_ffff);
}

pub const MICROPHONE: Icon = [
    "................",
    "......kkkk......",
    ".....kwwwwk.....",
    ".....kwlwwk.....",
    ".....kwlwwk.....",
    ".....kwlwwk.....",
    "...kkkwwwwkkk...",
    "...kwkwwwwkwk...",
    "...kwkkkkkkwk...",
    "...kkwwkkwwkk...",
    "....kkwwwwkk....",
    "......kwwk......",
    "......kwwk......",
    "....kkkwwkkk....",
    "....kwwwwwwk....",
    "....kkkkkkkk....",
];

pub const SPEAKER: Icon = [
    "................",
    "................",
    ".......kk...k...",
    "......kwk....k..",
    ".....kwwk.k...k.",
    "..kkkwwwk..k..k.",
    "..kwwwwwk..k..k.",
    "..kwwwwwk..k..k.",
    "..kwwwwwk..k..k.",
    "..kwwwwwk..k..k.",
    "..kkkwwwk..k..k.",
    ".....kwwk.k...k.",
    "......kwk....k..",
    ".......kk...k...",
    "................",
    "................",
];

pub const VOLUMES: Icon = [
    "................",
    "..kk............",
    ".kwwk...........",
    "kkwwkkkkkkkkkkk.",
    "kwwwwwwwwwwwwwk.",
    "kkwwkkkkkkkkkkk.",
    ".kwwk...........",
    "..kk.....kk.....",
    "........kwwk....",
    ".kkkkkkkkwwkkkk.",
    ".kwwwwwwwwwwwwwk",
    ".kkkkkkkkwwkkkk.",
    "........kwwk....",
    ".........kk.....",
    "................",
    "................",
];

pub const EYE: Icon = [
    "................",
    "................",
    "................",
    ".....kkkkkk.....",
    "...kkwwwwwwkk...",
    "..kwwwkkkkwwwk..",
    ".kwwwkbbbbkwwwk.",
    "kwwwkbbkkbbkwwwk",
    "kwwwkbbkkbbkwwwk",
    ".kwwwkbbbbkwwwk.",
    "..kwwwkkkkwwwk..",
    "...kkwwwwwwkk...",
    ".....kkkkkk.....",
    "................",
    "................",
    "................",
];

pub const LOCK: Icon = [
    "................",
    "................",
    ".....kkkkkk.....",
    "....kwwwwwwk....",
    "...kwwkkkkwwk...",
    "...kwk....kwk...",
    "...kwk....kwk...",
    "..kkkkkkkkkkkk..",
    "..kyyyyyyyyyyk..",
    "..kyyyyyyyyyyk..",
    "..kyyyykkyyyyk..",
    "..kyyyykkyyyyk..",
    "..kyyyyyyyyyyk..",
    "..kyyyyyyyyyyk..",
    "..kkkkkkkkkkkk..",
    "................",
];

pub const LEAVE: Icon = [
    "................",
    ".kkkkkkkk.......",
    ".kwwwwwwk.......",
    ".kwkkkkwk.......",
    ".kwk..kwk...k...",
    ".kwk..kwk...kk..",
    ".kwk..kwkkkkkrk.",
    ".kwk..kwrrrrrrrk",
    ".kwk..kwrrrrrrrk",
    ".kwk..kwkkkkkrk.",
    ".kwk..kwk...kk..",
    ".kwk..kwk...k...",
    ".kwkkkkwk.......",
    ".kwwwwwwk.......",
    ".kkkkkkkk.......",
    "................",
];

pub const GROUP: Icon = [
    "................",
    "................",
    ".kkkkkk.........",
    ".khhhhk..kkkkkk.",
    ".khhhhk..kddddk.",
    ".kssssk..kddddk.",
    ".kbsbsk..kssssk.",
    ".kssssk..kbsbsk.",
    ".kkkkkk..kssssk.",
    "kcccccck.kkkkkk.",
    "kcccccckkggggggk",
    "kcccccckkggggggk",
    "kkkkkkkkkggggggk",
    "........kggggggk",
    "........kkkkkkkk",
    "................",
];

pub const DISCONNECTED: Icon = [
    "................",
    ".....kk..kk.....",
    ".....kk..kk.....",
    ".....kk..kk.....",
    "...kkkkkkkkkk...",
    "...kwwwwwwwwk...",
    "...kwwwwwwwwk...",
    "...kwwwwwwwwk...",
    "....kwwwwwwk....",
    ".....kwwwwk.....",
    "......kwwk......",
    "......kwwk......",
    ".......kk.......",
    ".......kk.......",
    ".......kk.......",
    "................",
];

pub const TALK: Icon = [
    "kkkkkkkkkk......",
    "kGGGGGGGGk......",
    "kG......Gk......",
    "kG......Gk......",
    "kG......Gk......",
    "kG......Gk......",
    "kG......Gk......",
    "kG......Gk......",
    "kGGGGGGGGk......",
    "kkkkkkkkkk......",
    "................",
    "................",
    "................",
    "................",
    "................",
    "................",
];

pub const SLASH: Icon = [
    "................",
    ".............rr.",
    "............rrr.",
    "...........rrr..",
    "..........rrr...",
    ".........rrr....",
    "........rrr.....",
    ".......rrr......",
    "......rrr.......",
    ".....rrr........",
    "....rrr.........",
    "...rrr..........",
    "..rrr...........",
    ".rrr............",
    ".rr.............",
    "................",
];
