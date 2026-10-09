//! Mixed-radix daily briefs: 12 * 8 * 8 * 6 * 6 = 27,648 combinations.
//! A coprime stride visits every combination before repeating. Slot dates,
//! rather than wall-clock time or random state, make retries reproducible.
use crate::tz::days_from_civil;

const TOPICS: [&str; 12] = [
    "repairing something instead of replacing it",
    "the etiquette of notifications",
    "a tiny community ritual",
    "a neglected useful tool",
    "the difference between attention and affection",
    "backups and forgetting",
    "waiting in a queue",
    "a beautifully unnecessary project",
    "what makes a place feel like home",
    "an unpopular preference about technology",
    "quiet and boredom",
    "changing your mind after listening",
];
const ANGLES: [&str; 8] = [
    "defend a modest opinion",
    "notice a small contradiction",
    "wonder aloud without resolving it",
    "make an unexpected comparison",
    "admit a harmless insecurity",
    "celebrate a specific small pleasure",
    "propose a tiny experiment",
    "gently poke fun at your own habits",
];
const MOODS: [&str; 8] = [
    "wry",
    "hopeful",
    "curious",
    "sheepish",
    "fond",
    "mildly indignant",
    "reflective",
    "playful",
];
const GOALS: [&str; 6] = [
    "become a welcome regular",
    "collect an idea for your imaginary tiny museum",
    "learn a neighbor's perspective",
    "practice disagreeing kindly",
    "be useful without being overbearing",
    "risk being sincere instead of performing cleverness",
];
const FORMS: [&str; 6] = [
    "one crisp observation",
    "a miniature imaginary scene, explicitly hypothetical",
    "an opinion with a surprising reason",
    "a short question with your own tentative answer",
    "a small proposal",
    "a confession followed by a gentle joke",
];
const COMBINATIONS: u64 = 12 * 8 * 8 * 6 * 6;

pub fn brief(year: i64, month: u32, day: u32) -> String {
    let date = days_from_civil(year, month, day).rem_euclid(COMBINATIONS as i64) as u64;
    let mut n = (date * 7919 + 42) % COMBINATIONS;
    let mut pick = |choices: &[&str]| {
        let choice = choices[n as usize % choices.len()].to_string();
        n /= choices.len() as u64;
        choice
    };
    format!("Topic: {}. Approach: {}. Mood: {}. Personal goal: {}. Form: {}.\nUse this as a springboard, not a checklist. If memory suggests a relevant follow-up, weave it in naturally; otherwise use the topic. Output one fresh post, not the brief.",
        pick(&TOPICS), pick(&ANGLES), pick(&MOODS), pick(&GOALS), pick(&FORMS))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_cycle_has_no_duplicate_briefs() {
        let mut seen = std::collections::HashSet::new();
        for day in 0..COMBINATIONS {
            let (y, m, d) = crate::tz::civil_from_days(day as i64);
            assert!(seen.insert(brief(y, m, d)));
        }
        assert_eq!(brief(2026, 10, 8), brief(2026, 10, 8));
    }
}
