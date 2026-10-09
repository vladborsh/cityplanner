use std::fmt;

/// One content problem, naming the file and (when known) the field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentError {
    pub file: String,
    pub field: Option<String>,
    pub message: String,
}

impl ContentError {
    pub fn new(file: &str, field: Option<&str>, message: impl Into<String>) -> Self {
        Self {
            file: file.to_owned(),
            field: field.map(str::to_owned),
            message: message.into(),
        }
    }
}

impl fmt::Display for ContentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.field {
            Some(field) => write!(f, "{}: {}: {}", self.file, field, self.message),
            None => write!(f, "{}: {}", self.file, self.message),
        }
    }
}

impl std::error::Error for ContentError {}

/// All problems found while loading a content set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentErrors(pub Vec<ContentError>);

impl fmt::Display for ContentErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, e) in self.0.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{e}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ContentErrors {}
