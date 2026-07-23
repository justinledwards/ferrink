//! Bounded literary-excerpt corpus used by the optional home-screen clock.

use std::fmt;
use std::fs::File;
use std::io::{Cursor, Read};
use std::ops::Range;
use std::path::Path;

use csv::{ReaderBuilder, StringRecord};
use slint::StyledText;

const MINUTES_PER_DAY: usize = 24 * 60;
const MAX_CORPUS_BYTES: u64 = 2 * 1_048_576;
const MAX_CORPUS_RECORDS: usize = 5_000;
const MAX_TIME_PHRASE_CHARACTERS: usize = 80;
const MAX_QUOTE_CHARACTERS: usize = 800;
const MAX_CREDIT_FIELD_CHARACTERS: usize = 160;

/// One presentation-ready, attributed excerpt.
#[derive(Debug, Clone, PartialEq)]
pub struct LiteraryExcerpt {
    styled_text: StyledText,
    credit: String,
    font_size: u8,
}

impl LiteraryExcerpt {
    /// Returns the Markdown-parsed excerpt with only its time phrase emphasized.
    #[must_use]
    pub fn styled_text(&self) -> &StyledText {
        &self.styled_text
    }

    /// Returns the short work and author attribution.
    #[must_use]
    pub fn credit(&self) -> &str {
        self.credit.as_str()
    }

    /// Returns the bounded logical font size selected for the excerpt length.
    #[must_use]
    pub const fn font_size(&self) -> u8 {
        self.font_size
    }
}

/// Every bounded excerpt grouped by minute of day.
#[derive(Debug)]
pub struct LiteraryClockCorpus {
    entries: Vec<Vec<LiteraryExcerpt>>,
    entry_count: usize,
    covered_minutes: usize,
}

impl LiteraryClockCorpus {
    /// Loads an optional corpus from one exact regular file.
    ///
    /// A missing path is not an error because this home-screen feature is optional.
    /// Existing symlinks, non-files, oversized files, malformed records, and an empty
    /// usable corpus fail closed.
    pub fn load_optional(path: &Path) -> Result<Option<Self>, LiteraryClockError> {
        let metadata = match path.symlink_metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(LiteraryClockError::Io(error)),
        };
        if !metadata.file_type().is_file()
            || metadata.len() == 0
            || metadata.len() > MAX_CORPUS_BYTES
        {
            return Err(LiteraryClockError::InvalidCorpus(
                "corpus must be a non-empty regular file no larger than 2 MiB",
            ));
        }

        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
        File::open(path)
            .map_err(LiteraryClockError::Io)?
            .take(MAX_CORPUS_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(LiteraryClockError::Io)?;
        if bytes.len() as u64 > MAX_CORPUS_BYTES {
            return Err(LiteraryClockError::InvalidCorpus(
                "corpus grew beyond the 2 MiB read bound",
            ));
        }
        Self::from_pipe_separated(bytes.as_slice()).map(Some)
    }

    /// Parses the documented six-field pipe-separated corpus format.
    pub fn from_pipe_separated(input: &[u8]) -> Result<Self, LiteraryClockError> {
        if input.is_empty() || input.len() as u64 > MAX_CORPUS_BYTES {
            return Err(LiteraryClockError::InvalidCorpus(
                "corpus must contain at most 2 MiB",
            ));
        }

        let mut entries: Vec<Vec<LiteraryExcerpt>> = std::iter::repeat_with(Vec::new)
            .take(MINUTES_PER_DAY)
            .collect();
        let mut entry_count = 0_usize;
        let mut covered_minutes = 0_usize;
        let mut reader = ReaderBuilder::new()
            .delimiter(b'|')
            .has_headers(false)
            .flexible(false)
            .from_reader(Cursor::new(input));

        for (record_index, result) in reader.records().enumerate() {
            if record_index >= MAX_CORPUS_RECORDS {
                return Err(LiteraryClockError::InvalidCorpus(
                    "corpus contains more than 5,000 records",
                ));
            }
            let record = result.map_err(LiteraryClockError::Csv)?;
            let (minute, excerpt) = parse_record(&record)?;
            if entries[minute].is_empty() {
                covered_minutes += 1;
            }
            entries[minute].push(excerpt);
            entry_count += 1;
        }

        if entry_count == 0 {
            return Err(LiteraryClockError::InvalidCorpus(
                "corpus contains no usable excerpts",
            ));
        }
        Ok(Self {
            entries,
            entry_count,
            covered_minutes,
        })
    }

    /// Returns one deterministic excerpt registered for an exact 24-hour `HH:MM` key.
    ///
    /// The selector lets the caller rotate through every excerpt for a minute.
    #[must_use]
    pub fn excerpt_at(&self, time: &str, selector: u64) -> Option<&LiteraryExcerpt> {
        let entries = self.entries.get(minute_index(time)?)?;
        if entries.is_empty() {
            return None;
        }
        let count = u64::try_from(entries.len()).ok()?;
        let index = usize::try_from(selector % count).ok()?;
        entries.get(index)
    }

    /// Returns the number of minutes that have a usable excerpt.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entry_count
    }

    /// Reports whether the corpus has no usable entries.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entry_count == 0
    }

    /// Returns the number of clock minutes represented by at least one excerpt.
    #[must_use]
    pub const fn covered_minutes(&self) -> usize {
        self.covered_minutes
    }
}

/// A corpus could not be read or normalized safely.
#[derive(Debug)]
pub enum LiteraryClockError {
    /// File access failed.
    Io(std::io::Error),
    /// Pipe-separated record decoding failed.
    Csv(csv::Error),
    /// The corpus shape or content violated a fixed bound.
    InvalidCorpus(&'static str),
    /// Slint rejected the escaped inline emphasis representation.
    StyledText(slint::StyledTextFromMarkdownError),
}

impl fmt::Display for LiteraryClockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "cannot read literary corpus: {error}"),
            Self::Csv(error) => write!(formatter, "cannot parse literary corpus: {error}"),
            Self::InvalidCorpus(message) => formatter.write_str(message),
            Self::StyledText(error) => write!(formatter, "cannot style literary excerpt: {error}"),
        }
    }
}

impl std::error::Error for LiteraryClockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Csv(error) => Some(error),
            Self::StyledText(error) => Some(error),
            Self::InvalidCorpus(_) => None,
        }
    }
}

fn parse_record(record: &StringRecord) -> Result<(usize, LiteraryExcerpt), LiteraryClockError> {
    if record.len() != 6 {
        return Err(LiteraryClockError::InvalidCorpus(
            "every literary record must contain exactly six fields",
        ));
    }
    if !matches!(record.get(5), Some("NO" | "YES")) {
        return Err(LiteraryClockError::InvalidCorpus(
            "literary record content flag must be YES or NO",
        ));
    }

    let time = field(record, 0)?.trim();
    let phrase = field(record, 1)?.trim();
    let quote = field(record, 2)?.trim();
    let title = field(record, 3)?.trim();
    let author = field(record, 4)?.trim();
    let Some(minute) = minute_index(time) else {
        return Err(LiteraryClockError::InvalidCorpus(
            "literary record time must use 24-hour HH:MM",
        ));
    };

    if !bounded_clean_text(phrase, MAX_TIME_PHRASE_CHARACTERS)
        || !bounded_optional_text(title, MAX_CREDIT_FIELD_CHARACTERS)
        || !bounded_optional_text(author, MAX_CREDIT_FIELD_CHARACTERS)
    {
        return Err(LiteraryClockError::InvalidCorpus(
            "literary record metadata is oversized or contains controls",
        ));
    }
    if !bounded_clean_text(quote, MAX_QUOTE_CHARACTERS) {
        return Err(LiteraryClockError::InvalidCorpus(
            "literary excerpt is empty, exceeds 800 characters, or contains controls",
        ));
    }

    let Some(phrase_range) = find_ascii_case_insensitive(quote, phrase) else {
        return Err(LiteraryClockError::InvalidCorpus(
            "literary excerpt does not contain its declared time phrase",
        ));
    };
    let markdown = emphasized_markdown(quote, phrase_range);
    let styled_text =
        StyledText::from_markdown(markdown.as_str()).map_err(LiteraryClockError::StyledText)?;
    let credit = match (title.is_empty(), author.is_empty()) {
        (false, false) => format!("— {title}, {author}"),
        (false, true) => format!("— {title}"),
        (true, false) => format!("— {author}"),
        (true, true) => "— Source unavailable".to_owned(),
    };
    let font_size = excerpt_font_size(quote.chars().count());
    Ok((
        minute,
        LiteraryExcerpt {
            styled_text,
            credit,
            font_size,
        },
    ))
}

fn field(record: &StringRecord, index: usize) -> Result<&str, LiteraryClockError> {
    record.get(index).ok_or(LiteraryClockError::InvalidCorpus(
        "literary record field is missing",
    ))
}

fn minute_index(time: &str) -> Option<usize> {
    if time.len() != 5 || time.as_bytes().get(2) != Some(&b':') {
        return None;
    }
    let hour = time.get(..2)?.parse::<usize>().ok()?;
    let minute = time.get(3..)?.parse::<usize>().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    hour.checked_mul(60)?.checked_add(minute)
}

fn bounded_clean_text(value: &str, maximum_characters: usize) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.chars().count() <= maximum_characters
        && !value.chars().any(char::is_control)
}

fn bounded_optional_text(value: &str, maximum_characters: usize) -> bool {
    value.trim() == value
        && value.chars().count() <= maximum_characters
        && !value.chars().any(char::is_control)
}

const fn excerpt_font_size(character_count: usize) -> u8 {
    match character_count {
        0..=240 => 30,
        241..=420 => 26,
        421..=600 => 23,
        _ => 20,
    }
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<Range<usize>> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.char_indices().find_map(|(start, _)| {
        let end = start.checked_add(needle.len())?;
        haystack
            .is_char_boundary(end)
            .then(|| haystack.get(start..end))
            .flatten()
            .filter(|candidate| candidate.eq_ignore_ascii_case(needle))
            .map(|_| start..end)
    })
}

fn emphasized_markdown(quote: &str, phrase: Range<usize>) -> String {
    let mut markdown = String::with_capacity(quote.len() + 8);
    escape_markdown(&mut markdown, &quote[..phrase.start]);
    markdown.push_str("**");
    escape_markdown(&mut markdown, &quote[phrase.clone()]);
    markdown.push_str("**");
    escape_markdown(&mut markdown, &quote[phrase.end..]);
    markdown
}

fn escape_markdown(output: &mut String, value: &str) {
    for character in value.chars() {
        if character.is_ascii_punctuation() {
            output.push('\\');
        }
        output.push(character);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_safe_excerpt_and_emphasizes_only_the_time_phrase() {
        let input = b"09:41|nine forty-one|At nine forty-one, [Ada] closed *one* book.|Notes|A. Reader|NO\n";
        let corpus = LiteraryClockCorpus::from_pipe_separated(input)
            .expect("bounded safe corpus should parse");
        let excerpt = corpus
            .excerpt_at("09:41", 0)
            .expect("registered minute should be present");
        let expected = StyledText::from_markdown(
            "At **nine forty\\-one**, \\[Ada\\] closed \\*one\\* book\\.",
        )
        .expect("test markdown should parse");

        assert_eq!(excerpt.styled_text(), &expected);
        assert_eq!(excerpt.credit(), "— Notes, A. Reader");
        assert_eq!(excerpt.font_size(), 30);
        assert_eq!(corpus.len(), 1);
    }

    #[test]
    fn retains_every_record_and_rotates_same_minute_excerpts() {
        let input = b"09:41|09:41|First 09:41 excerpt.|Work A|Author A|NO\n09:41|nine forty-one|Second nine forty-one excerpt.|Work B|Author B|YES\n";
        let corpus = LiteraryClockCorpus::from_pipe_separated(input)
            .expect("complete bounded corpus should parse");

        assert_eq!(corpus.len(), 2);
        assert_eq!(corpus.covered_minutes(), 1);
        assert_eq!(
            corpus.excerpt_at("09:41", 0).unwrap().credit(),
            "— Work A, Author A"
        );
        assert_eq!(
            corpus.excerpt_at("09:41", 1).unwrap().credit(),
            "— Work B, Author B"
        );
        assert_eq!(
            corpus.excerpt_at("09:41", 2).unwrap().credit(),
            "— Work A, Author A"
        );
    }

    #[test]
    fn malformed_or_empty_corpora_fail_closed() {
        assert!(LiteraryClockCorpus::from_pipe_separated(b"").is_err());
        assert!(
            LiteraryClockCorpus::from_pipe_separated(b"24:00|midnight|midnight|A|B|NO\n").is_err()
        );
        assert!(LiteraryClockCorpus::from_pipe_separated(b"09:41|09:41|09:41|A|B\n").is_err());
        assert!(
            LiteraryClockCorpus::from_pipe_separated(b"09:41|09:41|09:41|A|B|MAYBE\n").is_err()
        );
    }

    #[test]
    fn long_excerpts_use_a_readable_bounded_font_scale() {
        assert_eq!(excerpt_font_size(16), 30);
        assert_eq!(excerpt_font_size(241), 26);
        assert_eq!(excerpt_font_size(421), 23);
        assert_eq!(excerpt_font_size(760), 20);
    }
}
