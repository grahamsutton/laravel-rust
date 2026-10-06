//! Lotteries: run a callback according to a set of odds.
//!
//! ```
//! use illuminate_support::Lottery;
//!
//! let mut lottery = Lottery::odds(1, 20)
//!     .winner(|| "You won!")
//!     .loser(|| "Better luck next time.");
//!
//! let result = lottery.choose();
//! assert!(result == "You won!" || result == "Better luck next time.");
//! ```
//!
//! Tests can force the outcome with [`Lottery::always_win`],
//! [`Lottery::always_lose`] and [`Lottery::fix`]. Forced results apply to
//! the current thread only, so parallel tests never interfere.

use std::cell::RefCell;
use std::collections::VecDeque;

use rand::Rng;

type ResultFactory = Box<dyn FnMut(Odds) -> bool>;

thread_local! {
    static RESULT_FACTORY: RefCell<Option<ResultFactory>> = RefCell::new(None);
}

/// The odds of winning a [`Lottery`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Odds {
    /// `chances` out of `out_of` draws win.
    Fraction { chances: u32, out_of: u32 },
    /// The probability (0.0 - 1.0) of winning.
    Probability(f64),
}

impl Odds {
    fn draw(self) -> bool {
        let mut rng = rand::rng();
        match self {
            Odds::Fraction { chances, out_of } => rng.random_range(1..=out_of.max(1)) <= chances,
            Odds::Probability(p) => rng.random::<f64>() < p,
        }
    }
}

/// A lottery: a callback for winners, a callback for losers, and the odds.
pub struct Lottery<'a, R = bool> {
    odds: Odds,
    winner: Option<Box<dyn FnMut() -> R + 'a>>,
    loser: Option<Box<dyn FnMut() -> R + 'a>>,
    fallback: fn(bool) -> R,
}

impl<'a> Lottery<'a, bool> {
    /// Create a lottery where `chances` out of `out_of` draws win.
    pub fn odds(chances: u32, out_of: u32) -> Self {
        Self {
            odds: Odds::Fraction {
                chances,
                out_of: out_of.max(1),
            },
            winner: None,
            loser: None,
            fallback: |won| won,
        }
    }

    /// Create a lottery that wins with the given probability (clamped to 0.0 - 1.0).
    pub fn probability(probability: f64) -> Self {
        Self {
            odds: Odds::Probability(probability.clamp(0.0, 1.0)),
            winner: None,
            loser: None,
            fallback: |won| won,
        }
    }
}

impl<'a, R> Lottery<'a, R> {
    /// Set the winner callback (missing callbacks return `R::default()`).
    pub fn winner<W: Default>(self, callback: impl FnMut() -> W + 'a) -> Lottery<'a, W> {
        Lottery {
            odds: self.odds,
            winner: Some(Box::new(callback)),
            loser: None,
            fallback: |_| W::default(),
        }
    }

    /// Set the loser callback.
    pub fn loser(mut self, callback: impl FnMut() -> R + 'a) -> Self {
        self.loser = Some(Box::new(callback));
        self
    }

    /// The odds of this lottery.
    pub fn get_odds(&self) -> Odds {
        self.odds
    }

    /// Draw once: determine if this run wins.
    pub fn wins(&self) -> bool {
        let odds = self.odds;
        let forced = RESULT_FACTORY.with(|cell| {
            let mut factory = cell.borrow_mut().take()?;
            let result = factory(odds);
            let mut current = cell.borrow_mut();
            if current.is_none() {
                *current = Some(factory);
            }
            Some(result)
        });
        forced.unwrap_or_else(|| odds.draw())
    }

    /// Run the lottery, calling the winner or loser callback.
    pub fn choose(&mut self) -> R {
        let won = self.wins();
        let callback = if won { &mut self.winner } else { &mut self.loser };
        match callback {
            Some(callback) => callback(),
            None => (self.fallback)(won),
        }
    }

    /// Run the lottery the given number of times.
    pub fn choose_times(&mut self, times: usize) -> Vec<R> {
        (0..times).map(|_| self.choose()).collect()
    }

    /// Turn the lottery into a plain callback, handy for APIs that accept closures.
    pub fn into_callback(mut self) -> impl FnMut() -> R + 'a
    where
        R: 'a,
    {
        move || self.choose()
    }
}

impl Lottery<'_> {
    /// Force every lottery on this thread to win.
    pub fn always_win() {
        Self::set_result_factory(|_| true);
    }

    /// Force every lottery on this thread to lose.
    pub fn always_lose() {
        Self::set_result_factory(|_| false);
    }

    /// Force lotteries on this thread to return the given results in order,
    /// then return to normal behavior.
    ///
    /// ```
    /// use illuminate_support::Lottery;
    ///
    /// Lottery::fix(vec![true, false]);
    /// assert!(Lottery::odds(1, 1_000_000).wins());
    /// assert!(!Lottery::odds(1, 1).wins());
    /// Lottery::determine_results_normally();
    /// ```
    pub fn fix(sequence: Vec<bool>) {
        let mut sequence: VecDeque<bool> = sequence.into();
        Self::set_result_factory(move |odds| sequence.pop_front().unwrap_or_else(|| odds.draw()));
    }

    /// Alias of [`Lottery::fix`].
    pub fn force_result_with_sequence(sequence: Vec<bool>) {
        Self::fix(sequence);
    }

    /// Indicate that lottery results should be determined normally again.
    pub fn determine_results_normally() {
        RESULT_FACTORY.with(|cell| *cell.borrow_mut() = None);
    }

    /// Set the factory deciding whether lotteries on this thread win.
    pub fn set_result_factory(factory: impl FnMut(Odds) -> bool + 'static) {
        RESULT_FACTORY.with(|cell| *cell.borrow_mut() = Some(Box::new(factory)));
    }
}

impl<R> std::fmt::Debug for Lottery<'_, R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lottery").field("odds", &self.odds).finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn it_can_win_and_lose() {
        Lottery::always_win();
        let wins = Cell::new(0);
        let mut lottery = Lottery::odds(1, 2).winner(|| wins.set(wins.get() + 1)).loser(|| ());
        lottery.choose();
        lottery.choose_times(2);
        assert_eq!(wins.get(), 3);

        Lottery::always_lose();
        assert_eq!(Lottery::odds(1, 1).winner(|| "won").loser(|| "lost").choose(), "lost");
        assert!(!Lottery::odds(1, 1).choose());
        Lottery::determine_results_normally();
    }

    #[test]
    fn it_returns_booleans_without_callbacks() {
        Lottery::fix(vec![true, false, true]);
        let mut lottery = Lottery::odds(1, 2);
        assert_eq!(lottery.choose_times(3), vec![true, false, true]);
        Lottery::determine_results_normally();
        assert!(Lottery::odds(1, 1).choose());
        assert!(!Lottery::odds(0, 1).choose());
        assert!(Lottery::probability(1.0).wins());
        assert!(!Lottery::probability(0.0).wins());
    }

    #[test]
    fn missing_callbacks_return_the_default() {
        Lottery::always_lose();
        let mut lottery = Lottery::odds(1, 1).winner(|| 42);
        assert_eq!(lottery.choose(), 0);
        Lottery::always_win();
        let mut callback = Lottery::odds(1, 1).winner(|| 42).into_callback();
        assert_eq!(callback(), 42);
        Lottery::determine_results_normally();
    }

    #[test]
    fn the_odds_are_respected() {
        let mut lottery = Lottery::odds(1, 2);
        let wins = lottery.choose_times(2000).into_iter().filter(|w| *w).count();
        assert!((800..1200).contains(&wins), "{wins} wins out of 2000");
    }
}
