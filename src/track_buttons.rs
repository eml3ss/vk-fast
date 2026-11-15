//! Which small buttons a track shows beside its title: in the player and in
//! track lists, chosen separately in the Interface settings.
use crate::{i18n::Language, icons::Icon};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Like,
    Dislike,
    Mix,
    Download,
}

impl Button {
    /// In the settings, and in the player from left to right.
    pub const ALL: [Self; 4] = [Self::Like, Self::Dislike, Self::Mix, Self::Download];
    /// In a list row from left to right: the heart stays next to the time.
    pub const ROW: [Self; 4] = [Self::Download, Self::Mix, Self::Dislike, Self::Like];

    fn bit(self) -> u8 {
        match self {
            Self::Like => 1,
            Self::Dislike => 2,
            Self::Mix => 4,
            Self::Download => 8,
        }
    }

    pub fn label(self, language: Language) -> &'static str {
        match self {
            Self::Like => language.text("Добавить в мою музыку", "Add to My music"),
            Self::Dislike => language.text("Не нравится", "Dislike"),
            Self::Mix => language.text("Микс по треку", "Go to track mix"),
            Self::Download => language.text("Скачать", "Download"),
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Self::Like => Icon::Heart,
            Self::Dislike => Icon::Dislike,
            Self::Mix => Icon::Radio,
            Self::Download => Icon::Save,
        }
    }
}

/// A choice of buttons, kept in the preferences as bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Set(u8);

impl Set {
    /// A new install's player: the heart and Dislike.
    pub const PLAYER: Self = Self(1 | 2);
    /// A new install's track lists: only the heart.
    pub const LISTS: Self = Self(1);

    pub fn from_bits(bits: u8) -> Self {
        Self(bits & 0b1111)
    }
    pub fn bits(self) -> u8 {
        self.0
    }
    pub fn has(self, button: Button) -> bool {
        self.0 & button.bit() != 0
    }
    pub fn with(self, button: Button, shown: bool) -> Self {
        if shown {
            Self(self.0 | button.bit())
        } else {
            Self(self.0 & !button.bit())
        }
    }
    pub fn count(self) -> usize {
        self.0.count_ones() as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_installs_show_the_heart_and_dislike_in_the_player_and_the_heart_in_lists() {
        let player = Set::PLAYER;
        assert!(player.has(Button::Like) && player.has(Button::Dislike));
        assert!(!player.has(Button::Mix) && !player.has(Button::Download));
        let lists = Set::LISTS;
        assert!(lists.has(Button::Like) && lists.count() == 1);
        let set = player.with(Button::Mix, true);
        let changed = set
            .with(Button::Download, true)
            .with(Button::Dislike, false);
        assert_eq!(Set::from_bits(changed.bits()), changed);
        assert_eq!(changed.count(), 3);
        assert_eq!(Set::from_bits(0xff).count(), 4, "unknown bits are dropped");
    }
}
