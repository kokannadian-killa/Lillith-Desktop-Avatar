//! Bundled fonts (all SIL Open Font License 1.1, see `fonts/OFL-*.txt`).
//!
//! | Role | Font |
//! |---|---|
//! | Carved titles, tab labels | Cinzel Decorative Bold |
//! | Body text | Chakra Petch Regular / SemiBold |
//! | DemonOS labels, status line | Michroma |
//! | Terminal, paths, commands | JetBrains Mono |

pub const CINZEL_DECORATIVE_BOLD: &[u8] = include_bytes!("../fonts/CinzelDecorative-Bold.ttf");
pub const CHAKRA_PETCH_REGULAR: &[u8] = include_bytes!("../fonts/ChakraPetch-Regular.ttf");
pub const CHAKRA_PETCH_SEMIBOLD: &[u8] = include_bytes!("../fonts/ChakraPetch-SemiBold.ttf");
pub const MICHROMA: &[u8] = include_bytes!("../fonts/Michroma-Regular.ttf");
pub const JETBRAINS_MONO: &[u8] = include_bytes!("../fonts/JetBrainsMono.ttf");

/// `(file name, bytes)` for every bundled font.
pub const ALL: [(&str, &[u8]); 5] = [
    ("CinzelDecorative-Bold.ttf", CINZEL_DECORATIVE_BOLD),
    ("ChakraPetch-Regular.ttf", CHAKRA_PETCH_REGULAR),
    ("ChakraPetch-SemiBold.ttf", CHAKRA_PETCH_SEMIBOLD),
    ("Michroma-Regular.ttf", MICHROMA),
    ("JetBrainsMono.ttf", JETBRAINS_MONO),
];

/// License notice to show in an About page.
pub const LICENSE_NOTICE: &str = "Fonts: Cinzel Decorative (c) 2012 Natanael Gama; \
Chakra Petch (c) 2018 The Chakra Petch Project Authors; Michroma (c) 2011 The Michroma Project Authors; JetBrains Mono (c) 2020 The JetBrains Mono Project Authors. \
All under the SIL Open Font License 1.1.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_are_truetype() {
        for (name, bytes) in ALL {
            assert!(bytes.len() > 10_000, "{name} too small");
            let magic = &bytes[..4];
            assert!(
                magic == [0, 1, 0, 0] || magic == *b"true" || magic == *b"OTTO",
                "{name}: bad magic {magic:?}"
            );
        }
    }
}
