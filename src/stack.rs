use std::collections::BTreeMap;
use std::fmt::Write;
use std::iter::Chain;
use std::slice::Iter;
use std::str;

use chrono::{FixedOffset, NaiveDateTime};
use git2::{Commit, FileMode, Oid, Reference, Repository, Tree};

pub(crate) use crate::error::Error;

const MAX_PARENTS: usize = 16;

pub(crate) struct PatchDescriptor {
    pub oid: Oid,
}

pub(crate) struct Stack {
    pub prev: Option<Oid>,
    pub head: Oid,
    pub applied: Vec<String>,
    pub unapplied: Vec<String>,
    pub hidden: Vec<String>,
    pub patches: BTreeMap<String, PatchDescriptor>,
}

impl Stack {
    pub fn new(head: Oid) -> Self {
        Self {
            prev: None,
            head,
            applied: vec![],
            unapplied: vec![],
            hidden: vec![],
            patches: BTreeMap::new(),
        }
    }

    pub fn from_branch(repo: &Repository, branch_name: Option<&str>) -> Result<Self, Error> {
        let stack_ref = get_stack_ref(repo, branch_name)?;
        let stack_tree = stack_ref.peel_to_tree()?;
        Ok(Stack::from_tree(repo, &stack_tree)?)
    }

    fn from_tree(repo: &Repository, tree: &Tree) -> Result<Self, Error> {
        let stack_json = tree.get_name("stack.json");
        if let Some(stack_json) = stack_json {
            let stack_json_blob = stack_json.to_object(&repo)?.peel_to_blob()?;
            Self::from_stack_json(stack_json_blob.content())
        } else {
            Err(Error::StGitStackMetadataNotFound)
        }
    }

    fn from_stack_json(data: &[u8]) -> Result<Self, Error> {
        match serde_json::from_slice(data) {
            Ok(queue_state) => Ok(queue_state),
            Err(e) => Err(Error::JsonError { source: e }),
        }
    }

    pub fn all_patches(&self) -> AllPatchesIter {
        AllPatchesIter(
            self.applied
                .iter()
                .chain(self.unapplied.iter())
                .chain(self.hidden.iter()),
        )
    }

    pub fn top(&self) -> Oid {
        if let Some(patch_name) = self.applied.last() {
            self.patches[patch_name].oid
        } else {
            self.head
        }
    }

    pub fn commit(
        &self,
        repo: &Repository,
        update_ref: Option<&str>,
        message: &str,
    ) -> Result<Oid, Error> {
        let prev_state_tree = match self.prev {
            Some(previous) => {
                let prev_tree = repo.find_tree(previous)?;
                let prev_state = Self::from_tree(repo, &prev_tree)?;
                Some((prev_state, prev_tree))
            }
            None => None,
        };
        let state_tree = self.make_tree(repo, &prev_state_tree)?;
        let sig = repo.signature()?;

        let simplified_parents: Vec<Commit> = match self.prev {
            Some(prev_oid) => vec![repo.find_commit(prev_oid)?.parent(0)?],
            None => vec![],
        };
        let simplified_parents: Vec<&Commit> = simplified_parents.iter().collect();

        let simplified_parent = repo.commit(
            None,
            &sig,
            &sig,
            message,
            &state_tree,
            simplified_parents.as_slice(),
        )?;

        use std::collections::HashSet;
        let mut parent_set = HashSet::new();
        parent_set.insert(self.head);
        parent_set.insert(self.top());
        for patch_name in &self.unapplied {
            parent_set.insert(self.patches[patch_name].oid);
        }
        for patch_name in &self.hidden {
            parent_set.insert(self.patches[patch_name].oid);
        }

        if let Some(oid) = self.prev {
            parent_set.insert(oid);
            let (prev_state, _) = prev_state_tree.unwrap();
            for patch_name in prev_state.all_patches() {
                parent_set.remove(&prev_state.patches[patch_name].oid);
            }
        }

        let mut parent_oids: Vec<Oid> = parent_set.iter().copied().collect();

        while parent_oids.len() > MAX_PARENTS {
            let parent_group_oids: Vec<Oid> = parent_oids
                .drain(parent_oids.len() - MAX_PARENTS..parent_oids.len())
                .collect();
            let mut parent_group: Vec<Commit> = Vec::with_capacity(MAX_PARENTS);
            for oid in parent_group_oids {
                parent_group.push(repo.find_commit(oid)?);
            }
            let parent_group: Vec<&Commit> = parent_group.iter().collect();
            let group_oid = repo.commit(
                None,
                &sig,
                &sig,
                "parent grouping",
                &state_tree,
                &parent_group,
            )?;
            parent_oids.push(group_oid);
        }

        let mut parent_commits: Vec<Commit> = Vec::with_capacity(parent_oids.len() + 1);
        parent_commits.push(repo.find_commit(simplified_parent)?);
        for oid in parent_oids {
            parent_commits.push(repo.find_commit(oid)?);
        }
        let parent_commits: Vec<&Commit> = parent_commits.iter().collect();

        let commit_oid = repo.commit(
            update_ref,
            &sig,
            &sig,
            message,
            &state_tree,
            &parent_commits,
        )?;

        Ok(commit_oid)
    }

    fn make_tree<'repo>(
        &self,
        repo: &'repo Repository,
        prev_state_tree: &Option<(Self, Tree)>,
    ) -> Result<Tree<'repo>, Error> {
        let mut builder = repo.treebuilder(None)?;
        builder.insert(
            "stack.json",
            repo.blob(serde_json::to_string_pretty(self)?.as_bytes())?,
            i32::from(FileMode::Blob),
        )?;
        builder.insert(
            "patches",
            self.make_patches_tree(repo, prev_state_tree)?,
            i32::from(FileMode::Tree),
        )?;
        let tree_oid = builder.write()?;
        let tree = repo.find_tree(tree_oid)?;
        Ok(tree)
    }

    fn make_patches_tree(
        &self,
        repo: &Repository,
        prev_state_tree: &Option<(Self, Tree)>,
    ) -> Result<Oid, Error> {
        let mut builder = repo.treebuilder(None)?;
        for patch_name in self.all_patches() {
            let oid = self.patches[patch_name].oid;
            builder.insert(
                patch_name,
                self.make_patch_meta(repo, patch_name, &oid, prev_state_tree)?,
                i32::from(FileMode::Blob),
            )?;
        }
        Ok(builder.write()?)
    }

    fn make_patch_meta(
        &self,
        repo: &Repository,
        patch_name: &str,
        oid: &Oid,
        prev_state_tree: &Option<(Self, Tree)>,
    ) -> Result<Oid, Error> {
        if let Some((prev_state, prev_tree)) = prev_state_tree {
            // And oid for this patch == oid for same patch in prev state
            // And we find the patch meta blob for this patch in the previous meta tree
            // Then return the previous patch meta blob.
            if prev_state.all_patches().any(|prev_patch_name| {
                let prev_patch_oid = &prev_state.patches[prev_patch_name].oid;
                prev_patch_name == patch_name && prev_patch_oid == oid
            }) {
                let patch_meta_path = String::from("patches/") + patch_name;
                let patch_meta_path = std::path::Path::new(&patch_meta_path);
                if let Ok(prev_patch_entry) = prev_tree.get_path(patch_meta_path) {
                    return Ok(prev_patch_entry.id());
                }
            }
        }

        let commit = repo.find_commit(*oid)?;
        let parent = commit.parent(0)?;
        let commit_time = commit.time();
        let commit_datetime = NaiveDateTime::from_timestamp(commit_time.seconds(), 0);
        let commit_tz = FixedOffset::west(commit_time.offset_minutes() * 60);

        let mut patch_meta = String::with_capacity(1024);
        write!(
            patch_meta,
            "Bottom: {}\n\
             Top:    {}\n\
             Author: {}\n\
             Date:   {} {}\n",
            parent.tree_id(),
            commit.tree_id(),
            commit.author(),
            commit_datetime,
            commit_tz,
        )?;

        Ok(repo.blob(patch_meta.as_bytes())?)
    }
}

pub struct AllPatchesIter<'a>(Chain<Chain<Iter<'a, String>, Iter<'a, String>>, Iter<'a, String>>);

impl<'a> Iterator for AllPatchesIter<'a> {
    type Item = &'a String;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

impl<'de> serde::Deserialize<'de> for Stack {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        #[derive(serde::Deserialize)]
        struct RawPatchDescriptor {
            pub oid: String,
        }

        #[derive(serde::Deserialize)]
        struct RawStackState {
            pub version: i64,
            pub prev: Option<String>,
            pub head: String,
            pub applied: Vec<String>,
            pub unapplied: Vec<String>,
            pub hidden: Vec<String>,
            pub patches: BTreeMap<String, RawPatchDescriptor>,
        }

        let raw = RawStackState::deserialize(deserializer)?;

        if raw.version != 5 {
            return Err(D::Error::invalid_value(
                ::serde::de::Unexpected::Signed(raw.version),
                &"5",
            ));
        }

        let prev: Option<Oid> = match raw.prev {
            // Some(oid_str) => Some(Oid::from_str(&oid_str).map_err(D::Error::custom("invalid oid"))),
            Some(oid_str) => Some(Oid::from_str(&oid_str).unwrap()),
            None => None,
        };

        // let head: Oid = Oid::from_str(raw.head).map_err(D::Error::custom("invalid oid"))?;
        let head: Oid = Oid::from_str(&raw.head).unwrap();

        let mut patches = BTreeMap::new();
        for (patch_name, raw_patch_desc) in raw.patches {
            // let oid = Oid::from_str(raw_patch_desc.oid).map_err(D::Error::custom("invalid oid"))?;
            let oid = Oid::from_str(&raw_patch_desc.oid).unwrap();
            patches.insert(patch_name, PatchDescriptor { oid });
        }
        Ok(Stack {
            prev,
            head,
            applied: raw.applied,
            unapplied: raw.unapplied,
            hidden: raw.hidden,
            patches,
        })
    }
}

impl serde::Serialize for Stack {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(serde::Serialize)]
        struct RawPatchDescriptor {
            pub oid: String,
        }

        #[derive(serde::Serialize)]
        struct RawStackState {
            pub version: String,
            pub prev: Option<String>,
            pub head: String,
            pub applied: Vec<String>,
            pub unapplied: Vec<String>,
            pub hidden: Vec<String>,
            pub patches: BTreeMap<String, RawPatchDescriptor>,
        }

        let prev: Option<String> = self.prev.map(|oid| oid.to_string());
        let head: String = self.head.to_string();
        let mut patches = BTreeMap::new();
        for (patch_name, patch_desc) in &self.patches {
            patches.insert(
                patch_name.clone(),
                RawPatchDescriptor {
                    oid: patch_desc.oid.to_string(),
                },
            );
        }

        let raw = RawStackState {
            version: "5".into(),
            prev,
            head,
            applied: self.applied.clone(),
            unapplied: self.unapplied.clone(),
            hidden: self.hidden.clone(),
            patches,
        };

        raw.serialize(serializer)
    }
}

pub(crate) fn initialize<'repo>(
    repo: &'repo Repository,
    branch_name: Option<&str>,
) -> Result<(), Error> {
    let branch_ref = get_branch_ref(repo, branch_name)?;
    let branch_shorthand = branch_ref.shorthand().ok_or(Error::StGitNonUtf8Name)?;
    let stack_refname = stack_refname_from_branch_shorthand(branch_shorthand);
    if repo.find_reference(&stack_refname).is_ok() {
        return Err(Error::StGitStackAlreadyInitialized(branch_shorthand.into()));
    }
    let stack = Stack::new(repo.head()?.peel_to_commit()?.id());
    stack.commit(repo, Some(&stack_refname), "initialize")?;
    Ok(())
}

fn stack_refname_from_branch_shorthand(branch_shorthand: &str) -> String {
    format!("refs/stacks/{}", branch_shorthand)
}

fn get_branch_ref<'repo>(
    repo: &'repo Repository,
    branch_name: Option<&str>,
) -> Result<Reference<'repo>, Error> {
    if let Some(name) = branch_name {
        Ok(repo.resolve_reference_from_short_name(name)?)
    } else {
        let head = repo.head()?;
        if head.is_branch() {
            Ok(head)
        } else {
            Err(Error::HeadDetached)
        }
    }
}

fn get_stack_ref<'repo>(
    repo: &'repo Repository,
    branch_name: Option<&str>,
) -> Result<Reference<'repo>, Error> {
    let branch_ref = get_branch_ref(repo, branch_name)?;
    let branch_shorthand = branch_ref.shorthand().ok_or(Error::StGitNonUtf8Name)?;
    let stack_refname = stack_refname_from_branch_shorthand(branch_shorthand);
    repo.find_reference(&stack_refname)
        .map_err(|_| Error::StGitStackNotInitialized(branch_shorthand.into()))
}
