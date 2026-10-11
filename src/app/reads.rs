//! When a server read may be sent again. Callers pass the time and the jitter, so nothing here does I/O.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub(super) const TD_REFRESH_DELAY: Duration = Duration::from_secs(2);
pub(super) const READ_COOLDOWN: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
pub(super) struct TourneyRefresh {
    pub(super) changed: Instant,
    pub(super) due: Instant,
}

#[derive(Default)]
pub(super) struct Reads {
    pub(super) sent: BTreeMap<String, Instant>,
    /// Latest event to cover and the next eligible refresh time, independent of later events.
    pub(super) td_refresh: Option<TourneyRefresh>,
}

impl Reads {
    /// Records `key` as read at `now` unless it was read within the cooldown.
    pub(super) fn allow(&mut self, key: String, now: Instant) -> bool {
        self.sent.retain(|_, at| now.duration_since(*at) < READ_COOLDOWN);
        if self.sent.contains_key(&key) {
            return false;
        }
        self.sent.insert(key, now);
        true
    }

    /// /td learns a result after /os. Later events extend coverage, never the first deadline.
    pub(super) fn tourney_changed(&mut self, now: Instant, jitter: Duration) {
        self.td_refresh.get_or_insert(TourneyRefresh { changed: now, due: now + TD_REFRESH_DELAY + jitter }).changed =
            now;
    }

    pub(super) fn refresh_due(&self, now: Instant) -> bool {
        self.td_refresh.is_some_and(|pending| now >= pending.due)
    }

    /// The commands of each `wanted` read the pending event still needs, respecting its own cooldown.
    /// A read sent after the event is skipped; one still cooling down moves the refresh to its cooldown's end.
    pub(super) fn refresh(&mut self, wanted: Vec<(String, Vec<String>)>, now: Instant) -> Vec<String> {
        let pending = self.td_refresh.take().unwrap_or(TourneyRefresh { changed: now, due: now });
        let mut retry_at: Option<Instant> = None;
        let mut commands = Vec::new();
        for (key, read) in wanted {
            if let Some(&at) = self.sent.get(&key) {
                if at > pending.changed {
                    continue;
                }
                let due = at + READ_COOLDOWN;
                if due > now {
                    retry_at = Some(retry_at.map_or(due, |next| next.min(due)));
                    continue;
                }
            }
            if self.allow(key, now) {
                commands.extend(read);
            }
        }
        if let Some(due) = retry_at {
            self.td_refresh = Some(TourneyRefresh { due, ..pending });
        }
        commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);

    fn wanted(keys: &[&str]) -> Vec<(String, Vec<String>)> {
        keys.iter().map(|k| (k.to_string(), vec![format!("cmd {k}")])).collect()
    }

    #[test]
    fn allow_admits_a_key_once_per_cooldown_and_prunes_expired_keys() {
        let t0 = Instant::now();
        let mut r = Reads::default();
        assert!(r.allow("a".into(), t0));
        assert!(r.allow("b".into(), t0 + S));
        assert!(!r.allow("a".into(), t0 + READ_COOLDOWN - S));
        assert!(r.allow("a".into(), t0 + READ_COOLDOWN));
        assert_eq!(r.sent.get("b"), Some(&(t0 + S)), "b is still cooling down");
        assert!(r.allow("c".into(), t0 + S + READ_COOLDOWN));
        assert_eq!(r.sent.keys().collect::<Vec<_>>(), ["a", "c"], "b expired and was pruned");
    }

    #[test]
    fn a_burst_of_events_keeps_the_first_deadline() {
        let t0 = Instant::now();
        let mut r = Reads::default();
        r.tourney_changed(t0, S);
        r.tourney_changed(t0 + S, Duration::ZERO);
        let p = r.td_refresh.unwrap();
        assert_eq!(p.due, t0 + TD_REFRESH_DELAY + S);
        assert_eq!(p.changed, t0 + S);
        assert!(!r.refresh_due(p.due - S));
        assert!(r.refresh_due(p.due));
    }

    #[test]
    fn refresh_sends_reads_not_sent_since_the_event_and_finishes() {
        let t0 = Instant::now();
        let mut r = Reads::default();
        r.sent.insert("before".into(), t0 - READ_COOLDOWN);
        r.sent.insert("at".into(), t0 - READ_COOLDOWN);
        r.td_refresh = Some(TourneyRefresh { changed: t0 - READ_COOLDOWN, due: t0 });
        r.sent.insert("after".into(), t0);
        let cmds = r.refresh(wanted(&["new", "at", "after"]), t0 + S);
        assert_eq!(cmds, ["cmd new", "cmd at"], "a read sent exactly at the event still needs refreshing");
        assert!(r.td_refresh.is_none(), "a finished refresh does not poll");
        assert_eq!(r.sent.get("at"), Some(&(t0 + S)), "a refreshed read starts a new cooldown");
    }

    #[test]
    fn refresh_waits_for_the_earliest_cooldown_and_keeps_the_event() {
        let t0 = Instant::now();
        let mut r = Reads::default();
        r.sent.insert("a".into(), t0);
        r.sent.insert("b".into(), t0 + S);
        r.td_refresh = Some(TourneyRefresh { changed: t0 + S, due: t0 + S });
        let cmds = r.refresh(wanted(&["b", "a", "c"]), t0 + 2 * S);
        assert_eq!(cmds, ["cmd c"]);
        let p = r.td_refresh.unwrap();
        assert_eq!((p.changed, p.due), (t0 + S, t0 + READ_COOLDOWN));
        assert_eq!(r.refresh(wanted(&["b", "a"]), t0 + READ_COOLDOWN), ["cmd a"]);
        assert_eq!(r.td_refresh.unwrap().due, t0 + S + READ_COOLDOWN, "b's cooldown ends exactly at due, which sends");
        assert_eq!(r.refresh(wanted(&["b"]), t0 + S + READ_COOLDOWN), ["cmd b"]);
        assert!(r.td_refresh.is_none());
    }

    #[test]
    fn refresh_without_an_event_covers_now() {
        let t0 = Instant::now();
        let mut r = Reads::default();
        r.sent.insert("recent".into(), t0);
        assert!(r.refresh(wanted(&["recent"]), t0 + S).is_empty());
        assert_eq!(r.td_refresh.map(|p| (p.changed, p.due)), Some((t0 + S, t0 + READ_COOLDOWN)));
    }
}
