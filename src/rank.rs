//! Ranking sessions for a query.
//!
//! Results are ordered by tier first, then by score:
//!   1. the whole query appears as an exact phrase (for one word: as a whole word),
//!   2. how many distinct query words appear,
//!   3. a field-weighted BM25 score. Each field saturates on its own, so
//!      repeated mentions keep helping with diminishing returns, but 300 hits
//!      in tool output can never outweigh one hit in the title.
//!
//! The last query word also matches as a prefix ("time" finds "timeout"),
//! so results stay useful while typing, but prefix-only hits rank below
//! whole-word hits.

use crate::extract::{Field, FIELDS, NUM_FIELDS};
use crate::index::Doc;
use crate::text::{self, Hits, Pattern};
use rayon::prelude::*;

pub const WEIGHTS: [f64; NUM_FIELDS] = {
    let mut w = [0.0; NUM_FIELDS];
    w[Field::Title as usize] = 12.0;
    w[Field::Project as usize] = 8.0;
    w[Field::User as usize] = 4.0;
    w[Field::Assistant as usize] = 2.0;
    w[Field::Other as usize] = 0.75;
    w[Field::ToolInput as usize] = 0.5;
    w[Field::ToolOutput as usize] = 0.25;
    w
};

const K1: f64 = 2.0;
/// Length normalization. Kept low: a long session that keeps coming back to
/// a topic is usually the one you want, not noise to discount.
const B: f64 = 0.3;
/// Sessions longer than this many times the average are not penalized further.
const MAX_LEN_RATIO: f64 = 4.0;
/// A word that only matched as a prefix counts this much of a whole-word hit.
const PREFIX_WEIGHT: f64 = 0.3;
/// The phrase is scored like an extra term, weighted up.
const PHRASE_BOOST: f64 = 2.0;
/// Recent sessions get at most this much extra, so recency only breaks near-ties.
const RECENCY_BOOST: f64 = 0.1;
const RECENCY_DAYS: f64 = 14.0;

pub struct Query {
    pub terms: Vec<String>,
    /// The last term may match as a prefix (the user may still be typing it).
    pub last_prefix: bool,
    term_patterns: Vec<Pattern>,
    phrase: Option<Pattern>,
}

impl Query {
    pub fn parse(raw: &str) -> Query {
        let mut terms = text::tokens(raw);
        let mut seen = std::collections::HashSet::new();
        let phrase = (terms.len() > 1).then(|| Pattern::new(&terms));
        let last_prefix = !terms.is_empty() && !raw.ends_with(char::is_whitespace);
        // Repeated words count once as terms; the phrase keeps the order as typed.
        let last = terms.last().cloned();
        terms.retain(|t| seen.insert(t.clone()));
        if let Some(last) = last {
            // Keep the typed-last word last, since only it gets prefix matching.
            terms.retain(|t| *t != last);
            terms.push(last);
        }
        let term_patterns = terms.iter().map(|t| Pattern::new(std::slice::from_ref(t))).collect();
        Query { terms, last_prefix, term_patterns, phrase }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    fn allows_prefix(&self, term: usize) -> bool {
        self.last_prefix && term + 1 == self.terms.len()
    }

    /// Per-field hits for every term and the phrase in one normalized text set.
    pub fn match_fields(&self, fields: &[String; NUM_FIELDS]) -> Match {
        let mut m = Match { terms: vec![[Hits::default(); NUM_FIELDS]; self.terms.len()], phrase: [Hits::default(); NUM_FIELDS] };
        for (f, text) in fields.iter().enumerate() {
            if text.is_empty() {
                continue;
            }
            for (i, p) in self.term_patterns.iter().enumerate() {
                let mut h = p.count(text);
                if !self.allows_prefix(i) {
                    h.prefix = 0;
                }
                m.terms[i][f] = h;
            }
            if let Some(p) = &self.phrase {
                let mut h = p.count(text);
                if !self.last_prefix {
                    h.prefix = 0;
                }
                m.phrase[f] = h;
            }
        }
        m
    }
}

pub struct Match {
    /// `terms[i][field]`
    pub terms: Vec<[Hits; NUM_FIELDS]>,
    pub phrase: [Hits; NUM_FIELDS],
}

fn any(h: &[Hits; NUM_FIELDS]) -> bool {
    h.iter().any(|h| h.whole + h.prefix > 0)
}

fn any_whole(h: &[Hits; NUM_FIELDS]) -> bool {
    h.iter().any(|h| h.whole > 0)
}

impl Match {
    /// Whole query found as typed: a phrase for several words, a whole word for one.
    pub fn exact(&self, q: &Query) -> bool {
        if q.terms.len() == 1 && q.phrase.is_none() {
            any_whole(&self.terms[0])
        } else {
            any(&self.phrase)
        }
    }

    pub fn matched_terms(&self) -> usize {
        self.terms.iter().filter(|t| any(t)).count()
    }
}

pub struct Ranked<'a> {
    pub doc: &'a Doc,
    pub exact: bool,
    pub matched: usize,
    pub score: f64,
    pub m: Match,
}

pub fn search<'a>(docs: &'a [Doc], q: &Query, now_secs: i64) -> Vec<Ranked<'a>> {
    if q.is_empty() {
        let mut all: Vec<Ranked> = docs
            .iter()
            .map(|doc| Ranked { doc, exact: false, matched: 0, score: 0.0, m: Match { terms: vec![], phrase: Default::default() } })
            .collect();
        all.sort_by(|a, b| (b.doc.mtime_secs, b.doc.mtime_nanos).cmp(&(a.doc.mtime_secs, a.doc.mtime_nanos)));
        return all;
    }

    let n = docs.len().max(1) as f64;
    let avg: [f64; NUM_FIELDS] =
        std::array::from_fn(|f| (docs.iter().map(|d| d.lens[f] as f64).sum::<f64>() / n).max(1.0));

    let matches: Vec<(&Doc, Match)> = docs
        .par_iter()
        .map(|d| (d, q.match_fields(&d.fields)))
        .filter(|(_, m)| m.matched_terms() > 0)
        .collect();

    let idf = |df: usize| (1.0 + (n - df as f64 + 0.5) / (df as f64 + 0.5)).ln();
    let term_idf: Vec<f64> = (0..q.terms.len()).map(|i| idf(matches.iter().filter(|(_, m)| any(&m.terms[i])).count())).collect();
    let phrase_idf = idf(matches.iter().filter(|(_, m)| any(&m.phrase)).count());

    let bm25 = |doc: &Doc, hits: &[Hits; NUM_FIELDS], idf: f64| {
        let per_field: f64 = FIELDS
            .iter()
            .map(|&f| {
                let i = f as usize;
                let tf = hits[i].whole as f64 + PREFIX_WEIGHT * hits[i].prefix as f64;
                let norm = 1.0 - B + B * (doc.lens[i] as f64 / avg[i]).min(MAX_LEN_RATIO);
                WEIGHTS[i] * tf * (K1 + 1.0) / (tf + K1 * norm)
            })
            .sum();
        idf * per_field
    };

    let mut ranked: Vec<Ranked> = matches
        .into_iter()
        .map(|(doc, m)| {
            let mut score: f64 = m.terms.iter().zip(&term_idf).map(|(h, &idf)| bm25(doc, h, idf)).sum();
            if q.phrase.is_some() {
                score += PHRASE_BOOST * bm25(doc, &m.phrase, phrase_idf);
            }
            let age_days = ((now_secs - doc.mtime_secs).max(0) as f64) / 86400.0;
            score *= 1.0 + RECENCY_BOOST * (-age_days / RECENCY_DAYS).exp();
            Ranked { doc, exact: m.exact(q), matched: m.matched_terms(), score, m }
        })
        .collect();

    ranked.sort_by(|a, b| {
        (b.exact, b.matched)
            .cmp(&(a.exact, a.matched))
            .then(b.score.total_cmp(&a.score))
            .then(b.doc.mtime_secs.cmp(&a.doc.mtime_secs))
    });
    ranked
}

/// Hit counts per field for display, e.g. "title proj you:3 claude:11".
pub fn badges(r: &Ranked) -> String {
    let mut parts = vec![];
    for f in FIELDS {
        let i = f as usize;
        let count: u32 = r.m.terms.iter().map(|t| t[i].whole + t[i].prefix).sum();
        if count == 0 {
            continue;
        }
        match f {
            Field::Title | Field::Project => parts.push(f.label().to_owned()),
            _ => parts.push(format!("{}:{}", f.label(), count)),
        }
    }
    let phrase: u32 = r.m.phrase.iter().map(|h| h.whole + h.prefix).sum();
    if phrase > 0 {
        parts.insert(0, format!("\"…\"×{phrase}"));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_dedupes_terms_and_tracks_prefix() {
        let q = Query::parse("Connection  timeout");
        assert_eq!(q.terms, ["connection", "timeout"]);
        assert!(q.last_prefix);
        assert!(!Query::parse("p0 ").last_prefix);
        assert_eq!(Query::parse("a b a").terms, ["b", "a"]);
    }
}
