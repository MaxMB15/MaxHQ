// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Which of Cursor's models can't read images. Cursor says whether it
//! takes images once for the whole process, not per model, and a model
//! that can't read them refuses every later request of a conversation
//! that has one. So the driver keeps its own list, by model ID, and sends
//! such a model an attached image by its path only.

/// How the IDs of models with no image input start.
const NO_IMAGES: &[&str] = &["glm-"];

/// Whether `model` reads images. A model the list doesn't name does, as
/// Auto does.
pub fn reads_images(model: &str) -> bool {
    !NO_IMAGES.iter().any(|start| model.starts_with(start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glm_models_cant_read_images() {
        for model in ["glm-5.2", "glm-5p3", "glm-5p3-flash", "glm-5p3-flash-high"] {
            assert!(!reads_images(model), "{model}");
        }
    }

    #[test]
    fn other_models_read_images() {
        for model in [
            "default",
            "composer-2.5",
            "claude-opus-5-5",
            "gpt-5.5",
            "gemini-3.1-pro",
            "grok-4.7",
            "not-glm-5",
        ] {
            assert!(reads_images(model), "{model}");
        }
    }
}
