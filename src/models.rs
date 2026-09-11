use chrono::{DateTime, Utc};
use rs_fsrs::{Card as FsrsCard, State as FsrsState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deck {
    pub id: i64,
    pub name: String,
    pub new_per_day: i64,
    pub rev_per_day: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeckCounts {
    pub deck_id: i64,
    pub name: String,
    pub new: i64,
    pub learning: i64,
    pub review: i64,
    pub total: i64,
    #[serde(default)]
    pub level: usize,
}

impl DeckCounts {
    pub fn due_total(&self) -> i64 {
        self.new + self.learning + self.review
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[repr(i32)]
pub enum CardState {
    New = 0,
    Learning = 1,
    Review = 2,
    Relearning = 3,
}

impl From<FsrsState> for CardState {
    fn from(value: FsrsState) -> Self {
        match value {
            FsrsState::New => Self::New,
            FsrsState::Learning => Self::Learning,
            FsrsState::Review => Self::Review,
            FsrsState::Relearning => Self::Relearning,
        }
    }
}

impl From<CardState> for FsrsState {
    fn from(value: CardState) -> Self {
        match value {
            CardState::New => Self::New,
            CardState::Learning => Self::Learning,
            CardState::Review => Self::Review,
            CardState::Relearning => Self::Relearning,
        }
    }
}

impl CardState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Learning => "Learning",
            Self::Review => "Review",
            Self::Relearning => "Relearning",
        }
    }

    pub fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::Learning,
            2 => Self::Review,
            3 => Self::Relearning,
            _ => Self::New,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Card {
    pub id: i64,
    pub note_id: i64,
    pub deck_id: i64,
    pub due: DateTime<Utc>,
    pub stability: f64,
    pub difficulty: f64,
    pub elapsed_days: i64,
    pub scheduled_days: i64,
    pub reps: i32,
    pub lapses: i32,
    pub state: CardState,
    pub last_review: DateTime<Utc>,
    pub suspended: bool,
    pub created_at: DateTime<Utc>,
}

impl Card {
    pub fn to_fsrs(&self) -> FsrsCard {
        FsrsCard {
            due: self.due,
            stability: self.stability,
            difficulty: self.difficulty,
            elapsed_days: self.elapsed_days,
            scheduled_days: self.scheduled_days,
            reps: self.reps,
            lapses: self.lapses,
            state: self.state.into(),
            last_review: self.last_review,
        }
    }

    pub fn apply_fsrs(&mut self, fsrs: &FsrsCard) {
        self.due = fsrs.due;
        self.stability = fsrs.stability;
        self.difficulty = fsrs.difficulty;
        self.elapsed_days = fsrs.elapsed_days;
        self.scheduled_days = fsrs.scheduled_days;
        self.reps = fsrs.reps;
        self.lapses = fsrs.lapses;
        self.state = fsrs.state.into();
        self.last_review = fsrs.last_review;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudyCard {
    pub card: Card,
    pub front: String,
    pub back: String,
    pub tags: String,
    pub deck_name: String,
    #[serde(default)]
    pub answer_intervals: Vec<String>,
    #[serde(default)]
    pub answer_includes_question: bool,
    #[serde(default)]
    pub front_document: Option<CardDocument>,
    #[serde(default)]
    pub back_document: Option<CardDocument>,
    #[serde(default)]
    pub scheduling_states_hex: String,
    #[serde(default)]
    pub fields: Vec<NoteField>,
}

/// Official template render for the Edit preview pane (draft fields, unsaved).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardPreviewRender {
    pub front: String,
    pub back: String,
    #[serde(default)]
    pub answer_includes_question: bool,
    #[serde(default)]
    pub front_document: Option<CardDocument>,
    #[serde(default)]
    pub back_document: Option<CardDocument>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CardDocument {
    #[serde(default)]
    pub background: String,
    #[serde(default)]
    pub blocks: Vec<CardBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CardBlock {
    Text {
        #[serde(default)]
        runs: Vec<CardTextRun>,
        #[serde(default)]
        alignment: String,
        #[serde(default)]
        background: String,
    },
    Separator,
    Image {
        source: String,
    },
    Audio {
        #[serde(default)]
        sources: Vec<String>,
    },
    Spacer,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CardTextRun {
    pub text: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    #[serde(default)]
    pub crossed_out: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct BrowseRow {
    pub card_id: i64,
    pub note_id: i64,
    pub deck_name: String,
    pub front: String,
    pub back: String,
    pub tags: String,
    pub state: CardState,
    pub due: DateTime<Utc>,
    pub reps: i32,
    pub lapses: i32,
    pub suspended: bool,
    #[serde(default)]
    pub due_label: String,
    #[serde(default)]
    pub fields: Vec<NoteField>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatsSummary {
    pub total_cards: i64,
    pub total_notes: i64,
    pub total_decks: i64,
    pub new_cards: i64,
    pub learning_cards: i64,
    pub review_cards: i64,
    pub relearning_cards: i64,
    pub suspended_cards: i64,
    pub reviews_today: i64,
    pub reviews_7d: i64,
    pub reviews_30d: i64,
    pub mature_cards: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeckOptions {
    pub new_per_day: i64,
    pub rev_per_day: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteField {
    pub name: String,
    pub value: String,
}
