//! `model/ranklib`: the model file RankLib saves for MART, LambdaMART and
//! Random Forests: `##` header lines, then an XML `<ensemble>` of `<tree>`s
//! whose `<split>`s hold a 1-based `<feature>`, a `<threshold>` and a
//! `pos="left"` and `pos="right"` child, ending in `<output>` leaves.

use super::{MAX_DEPTH, MAX_TREES, ModelError};

#[derive(Debug)]
enum Node {
    Leaf(f32),
    Split {
        feature: usize,
        threshold: f32,
        left: usize,
        right: usize,
    },
}

#[derive(Debug)]
struct Tree {
    weight: f32,
    nodes: Vec<Node>,
}

/// A parsed RankLib ensemble.
#[derive(Debug)]
pub struct Ranklib {
    /// One entry per `<ensemble>`: a Random Forest's bags, or the one
    /// ensemble of a boosted model.
    ensembles: Vec<Vec<Tree>>,
    /// Random Forests average their bags; boosted models have one.
    average: bool,
}

#[derive(Debug, PartialEq)]
enum Tag<'a> {
    Open { name: &'a str, attrs: &'a str },
    Close(&'a str),
    Text(&'a str),
}

/// A minimal XML reader: tags and text, no entities, comments skipped.
struct Reader<'a> {
    rest: &'a str,
}

impl<'a> Reader<'a> {
    fn next(&mut self) -> Result<Option<Tag<'a>>, ModelError> {
        loop {
            let rest = self.rest.trim_start();
            if rest.is_empty() {
                return Ok(None);
            }
            if let Some(after) = rest.strip_prefix("<!--") {
                let end = after.find("-->").ok_or_else(|| {
                    ModelError::new("the RankLib XML has an unterminated comment")
                })?;
                self.rest = &after[end.saturating_add(3)..];
                continue;
            }
            if let Some(after) = rest.strip_prefix("<?") {
                let end = after.find("?>").ok_or_else(|| {
                    ModelError::new("the RankLib XML has an unterminated declaration")
                })?;
                self.rest = &after[end.saturating_add(2)..];
                continue;
            }
            if let Some(after) = rest.strip_prefix('<') {
                let end = after
                    .find('>')
                    .ok_or_else(|| ModelError::new("the RankLib XML has an unterminated tag"))?;
                self.rest = &after[end.saturating_add(1)..];
                let inside = after[..end].trim();
                if let Some(name) = inside.strip_prefix('/') {
                    return Ok(Some(Tag::Close(name.trim())));
                }
                if inside.ends_with('/') {
                    return Err(ModelError::new("the RankLib XML has an empty element"));
                }
                let (name, attrs) = inside
                    .split_once(char::is_whitespace)
                    .unwrap_or((inside, ""));
                return Ok(Some(Tag::Open { name, attrs }));
            }
            let end = rest.find('<').unwrap_or(rest.len());
            self.rest = &rest[end..];
            return Ok(Some(Tag::Text(rest[..end].trim())));
        }
    }

    fn expect_close(&mut self, name: &str) -> Result<(), ModelError> {
        match self.next()? {
            Some(Tag::Close(n)) if n == name => Ok(()),
            // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
            _ => Err(ModelError::new(format!(
                "the RankLib XML expects </{name}>"
            ))),
        }
    }

    /// `<name> text </name>` read as a number, the open tag already read.
    fn number(&mut self, name: &str) -> Result<f32, ModelError> {
        let Some(Tag::Text(text)) = self.next()? else {
            // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
            return Err(ModelError::new(format!(
                "the RankLib XML expects a number in <{name}>"
            )));
        };
        let value = text
            .parse::<f32>()
            // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
            .map_err(|_| ModelError::new(format!("[{text}] in <{name}> is not a number")))?;
        self.expect_close(name)?;
        Ok(value)
    }
}

fn attr<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
    let at = attrs.find(&format!("{key}="))?;
    let after =
        attrs[at.saturating_add(key.len()).saturating_add(1)..].trim_start_matches(['"', '\'']);
    after.split(['"', '\'']).next()
}

impl Ranklib {
    pub(super) fn parse(definition: &str, features: usize) -> Result<Self, ModelError> {
        let mut ranker = None;
        let mut body = definition;
        // Header lines: `## <Ranker>` first, then its parameters.
        while let Some(line) = body.trim_start().strip_prefix("##") {
            let (head, rest) = line.split_once('\n').unwrap_or((line, ""));
            if ranker.is_none() {
                ranker = Some(head.trim());
            }
            body = rest;
        }
        let average = match ranker {
            None | Some("LambdaMART" | "MART") => false,
            Some("Random Forests") => true,
            Some(other) => {
                // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
                return Err(ModelError::new(format!(
                    "the RankLib ranker [{other}] is not supported; use MART, LambdaMART or Random Forests"
                )));
            }
        };
        let mut reader = Reader { rest: body };
        // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
        let mut ensembles = Vec::new();
        let mut trees_total = 0usize;
        while let Some(tag) = reader.next()? {
            match tag {
                Tag::Open {
                    name: "ensemble", ..
                } => {
                    let trees = ensemble(&mut reader, features, &mut trees_total)?;
                    ensembles.push(trees);
                }
                _ => return Err(ModelError::new("the RankLib model expects an <ensemble>")),
            }
        }
        if ensembles.is_empty() || ensembles.iter().all(Vec::is_empty) {
            return Err(ModelError::new("the RankLib model has no trees"));
        }
        if !average && ensembles.len() > 1 {
            return Err(ModelError::new(
                "a boosted RankLib model has one <ensemble>",
            ));
        }
        Ok(Self { ensembles, average })
    }

    pub(super) fn score(&self, vector: &[f32]) -> f32 {
        let sum: f32 = self
            .ensembles
            .iter()
            .map(|trees| trees.iter().map(|t| t.weight * t.leaf(vector)).sum::<f32>())
            .sum();
        if self.average {
            sum / self.ensembles.len() as f32
        } else {
            sum
        }
    }
}

impl Tree {
    fn leaf(&self, vector: &[f32]) -> f32 {
        let mut at = 0usize;
        for _ in 0..=MAX_DEPTH {
            match self.nodes.get(at) {
                Some(Node::Leaf(v)) => return *v,
                Some(Node::Split {
                    feature,
                    threshold,
                    left,
                    right,
                }) => {
                    let x = vector.get(*feature).copied().unwrap_or(0.0);
                    let x = if x.is_nan() { 0.0 } else { x };
                    at = if x <= *threshold { *left } else { *right };
                }
                None => return 0.0,
            }
        }
        0.0
    }
}

fn ensemble(
    reader: &mut Reader<'_>,
    features: usize,
    total: &mut usize,
) -> Result<Vec<Tree>, ModelError> {
    // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
    let mut trees = Vec::new();
    loop {
        match reader.next()? {
            Some(Tag::Close("ensemble")) => return Ok(trees),
            Some(Tag::Open {
                name: "tree",
                attrs,
            }) => {
                *total = total.saturating_add(1);
                if *total > MAX_TREES {
                    // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
                    return Err(ModelError::new(format!(
                        "the RankLib model has more than {MAX_TREES} trees"
                    )));
                }
                let weight = match attr(attrs, "weight") {
                    Some(w) => w.parse::<f32>().map_err(|_| {
                        // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
                        ModelError::new(format!("the tree weight [{w}] is not a number"))
                    })?,
                    None => 1.0,
                };
                // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
                let mut nodes = Vec::new();
                match reader.next()? {
                    Some(Tag::Open { name: "split", .. }) => {
                        node(reader, features, &mut nodes, 1)?;
                    }
                    _ => return Err(ModelError::new("a RankLib <tree> holds a <split>")),
                }
                reader.expect_close("tree")?;
                trees.push(Tree { weight, nodes });
            }
            _ => return Err(ModelError::new("an <ensemble> holds <tree>s")),
        }
    }
}

/// Reads the body of a `<split>` (its open tag read) into `nodes`, returning
/// its index.
fn node(
    reader: &mut Reader<'_>,
    features: usize,
    nodes: &mut Vec<Node>,
    depth: usize,
) -> Result<usize, ModelError> {
    if depth > MAX_DEPTH {
        // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
        return Err(ModelError::new(format!(
            "a RankLib tree is deeper than {MAX_DEPTH}"
        )));
    }
    let at = nodes.len();
    nodes.push(Node::Leaf(0.0));
    match reader.next()? {
        Some(Tag::Open { name: "output", .. }) => {
            let value = reader.number("output")?;
            nodes[at] = Node::Leaf(value);
        }
        Some(Tag::Open {
            name: "feature", ..
        }) => {
            let id = reader.number("feature")?;
            let feature = id as usize;
            if id.fract() != 0.0 || feature == 0 || feature > features {
                // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
                return Err(ModelError::new(format!(
                    "a RankLib split names the feature [{id}], but the feature set has {features}"
                )));
            }
            let Some(Tag::Open {
                name: "threshold", ..
            }) = reader.next()?
            else {
                return Err(ModelError::new("a RankLib <split> holds a <threshold>"));
            };
            let threshold = reader.number("threshold")?;
            let mut children = [None, None];
            for _ in 0..2 {
                let Some(Tag::Open {
                    name: "split",
                    attrs,
                }) = reader.next()?
                else {
                    return Err(ModelError::new(
                        "a RankLib <split> holds a left and a right <split>",
                    ));
                };
                let slot = match attr(attrs, "pos") {
                    Some("left") => 0,
                    Some("right") => 1,
                    _ => {
                        return Err(ModelError::new(
                            "a RankLib child <split> has pos left or right",
                        ));
                    }
                };
                if children[slot].is_some() {
                    return Err(ModelError::new(
                        "a RankLib <split> has two children on one side",
                    ));
                }
                children[slot] = Some(node(reader, features, nodes, depth.saturating_add(1))?);
            }
            let (Some(left), Some(right)) = (children[0], children[1]) else {
                return Err(ModelError::new("a RankLib <split> needs both children"));
            };
            nodes[at] = Node::Split {
                feature: feature.saturating_sub(1),
                threshold,
                left,
                right,
            };
        }
        _ => {
            return Err(ModelError::new(
                "a RankLib <split> holds an <output> or a <feature>",
            ));
        }
    }
    reader.expect_close("split")?;
    Ok(at)
}
