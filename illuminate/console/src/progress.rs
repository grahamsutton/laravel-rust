//! Progress bars for long running tasks.
//!
//! ```
//! use illuminate_console::{Output, ProgressBar};
//!
//! let output = Output::buffered();
//! let bar = ProgressBar::new(&output, 2);
//!
//! bar.start();
//! bar.advance();
//! bar.advance();
//! bar.finish();
//!
//! assert!(output.fetch().ends_with(" 2/2 [============================] 100%"));
//! ```

use std::sync::{Arc, Mutex};

use crate::output::Output;

struct State {
    max: usize,
    step: usize,
    started: bool,
    previous: Option<String>,
    message: Option<String>,
}

/// A progress bar, rendered like Symfony's.
#[derive(Clone)]
pub struct ProgressBar {
    output: Output,
    state: Arc<Mutex<State>>,
    width: usize,
}

impl ProgressBar {
    /// Create a progress bar with the given number of steps.
    pub fn new(output: &Output, max: usize) -> Self {
        Self {
            output: output.clone(),
            state: Arc::new(Mutex::new(State {
                max,
                step: 0,
                started: false,
                previous: None,
                message: None,
            })),
            width: 28,
        }
    }

    /// Start the progress bar, displaying it at zero.
    pub fn start(&self) {
        {
            let mut state = self.state.lock().unwrap();
            state.started = true;
            state.step = 0;
        }
        self.display();
    }

    /// Advance the progress bar by one step.
    pub fn advance(&self) {
        self.advance_by(1);
    }

    /// Advance the progress bar by the given number of steps.
    pub fn advance_by(&self, steps: usize) {
        let step = self.state.lock().unwrap().step + steps;
        self.set_progress(step);
    }

    /// Move the progress bar to the given step.
    pub fn set_progress(&self, step: usize) {
        {
            let mut state = self.state.lock().unwrap();
            if state.max > 0 && step > state.max {
                state.max = step;
            }
            state.step = step;
            state.started = true;
        }
        self.display();
    }

    /// Set a message displayed next to the bar.
    pub fn set_message(&self, message: impl Into<String>) {
        self.state.lock().unwrap().message = Some(message.into());
    }

    /// Change the number of steps.
    pub fn set_max_steps(&self, max: usize) {
        self.state.lock().unwrap().max = max;
    }

    /// Finish the progress bar, filling it completely.
    pub fn finish(&self) {
        let max = {
            let state = self.state.lock().unwrap();
            if state.max == 0 { state.step } else { state.max }
        };
        self.state.lock().unwrap().max = max;
        self.set_progress(max);
    }

    /// The current step.
    pub fn progress(&self) -> usize {
        self.state.lock().unwrap().step
    }

    /// The number of steps.
    pub fn max_steps(&self) -> usize {
        self.state.lock().unwrap().max
    }

    /// Render the bar's current line (without writing it).
    pub fn render(&self) -> String {
        let state = self.state.lock().unwrap();
        let mut line = if state.max > 0 {
            let percent = (state.step as f64 / state.max as f64).min(1.0);
            let complete = (percent * self.width as f64).floor() as usize;
            let mut bar = "=".repeat(complete);
            if complete < self.width {
                bar.push('>');
                bar.push_str(&"-".repeat(self.width - complete - 1));
            }
            format!(
                " {:>width$}/{} [{}] {:>3}%",
                state.step,
                state.max,
                bar,
                (percent * 100.0).floor() as usize,
                width = state.max.to_string().len()
            )
        } else {
            let position = state.step % self.width;
            let mut bar = "=".repeat(position);
            bar.push('>');
            bar.push_str(&"-".repeat(self.width - position - 1));
            format!(" {} [{}]", state.step, bar)
        };

        if let Some(message) = &state.message {
            line.push(' ');
            line.push_str(message);
        }

        line
    }

    fn display(&self) {
        let line = self.render();
        let mut state = self.state.lock().unwrap();

        if state.previous.as_deref() == Some(line.as_str()) {
            return;
        }

        if self.output.is_decorated() {
            if state.previous.is_some() {
                // Move to the first column and clear the line.
                self.output.write("\x1b[1G\x1b[2K");
            }
            self.output.write(&line);
        } else if state.previous.is_some() {
            self.output.write(format!("\n{line}"));
        } else {
            self.output.write(&line);
        }

        state.previous = Some(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_renders_progress() {
        let output = Output::buffered();
        let bar = ProgressBar::new(&output, 3);
        bar.start();
        bar.advance();
        bar.finish();

        assert_eq!(
            output.fetch(),
            " 0/3 [>---------------------------]   0%\n 1/3 [=========>------------------]  33%\n 3/3 [============================] 100%"
        );
    }

    #[test]
    fn it_redraws_in_place_when_decorated() {
        let output = Output::buffered().with_decoration(true);
        let bar = ProgressBar::new(&output, 2);
        bar.start();
        bar.advance();
        assert!(output.fetch().contains("\x1b[1G\x1b[2K 1/2"));
    }

    #[test]
    fn it_pads_the_current_step() {
        let output = Output::buffered();
        let bar = ProgressBar::new(&output, 10);
        bar.set_progress(5);
        assert_eq!(bar.render(), "  5/10 [==============>-------------]  50%");
    }
}
