use chrono::Utc;
use rs_fsrs::{Card as FsrsCard, Rating, FSRS};

use crate::models::Card;

#[derive(Clone)]
pub struct Scheduler {
    fsrs: FSRS,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            fsrs: FSRS::default(),
        }
    }

    pub fn review(&self, card: &Card, rating: Rating) -> (Card, Rating) {
        let fsrs_card = card.to_fsrs();
        let info = self.fsrs.next(fsrs_card, Utc::now(), rating);
        let mut updated = card.clone();
        updated.apply_fsrs(&info.card);
        (updated, rating)
    }

    pub fn preview_intervals(&self, card: &Card) -> [(Rating, String); 4] {
        let fsrs_card = card.to_fsrs();
        let log = self.fsrs.repeat(fsrs_card, Utc::now());
        [
            (Rating::Again, format_interval(&log[&Rating::Again].card)),
            (Rating::Hard, format_interval(&log[&Rating::Hard].card)),
            (Rating::Good, format_interval(&log[&Rating::Good].card)),
            (Rating::Easy, format_interval(&log[&Rating::Easy].card)),
        ]
    }
}

fn format_interval(card: &FsrsCard) -> String {
    use rs_fsrs::State;
    match card.state {
        State::Learning | State::Relearning => {
            let mins = ((card.due - Utc::now()).num_seconds().max(0) + 59) / 60;
            if mins < 60 {
                format!("{mins}m")
            } else {
                format!("{}h", (mins + 59) / 60)
            }
        }
        State::Review | State::New => {
            let days = card.scheduled_days.max(0);
            if days == 0 {
                "<1d".into()
            } else if days < 30 {
                format!("{days}d")
            } else if days < 365 {
                format!("{}mo", days / 30)
            } else {
                format!("{:.1}y", days as f64 / 365.0)
            }
        }
    }
}

pub fn rating_label(rating: Rating) -> &'static str {
    match rating {
        Rating::Again => "Again",
        Rating::Hard => "Hard",
        Rating::Good => "Good",
        Rating::Easy => "Easy",
    }
}

pub fn rating_key(rating: Rating) -> char {
    match rating {
        Rating::Again => '1',
        Rating::Hard => '2',
        Rating::Good => '3',
        Rating::Easy => '4',
    }
}
