use clap::{App, Arg, ArgMatches, ValueHint};
use git2::DiffOptions;

use crate::{
    argset,
    commit::CommitData,
    error::Error,
    patchdescription::PatchDescription,
    patchname::PatchName,
    signature,
    stack::{ConflictMode, Stack, StackStateAccess},
};

pub(super) fn get_command() -> (&'static str, super::StGitCommand) {
    ("new", super::StGitCommand { get_app, run })
}

fn get_app() -> App<'static> {
    let app = App::new("new")
        .about("Create a new patch at top of the stack")
        .long_about(
            "Create a new, empty patch on the current stack. The new \
             patch is created on top of the currently applied patches, \
             and is made the new top of the stack. Uncommitted changes \
             in the work tree are not included in the patch -- that is \
             handled by stg-refresh.\n\
             \n\
             The given name must be unique in the stack, and may only \
             contain alphanumeric characters, dashes and underscores. \
             If no name is given, one is generated from the first line \
             of the patch's commit message.\n\
             \n\
             An editor will be launched to edit the commit message to \
             be used for the patch, unless the '--message' flag \
             already specified one. The 'patchdescr.tmpl' template \
             file (if available) is used to pre-fill the editor.",
        )
        .arg(
            Arg::new("verbose")
                .long("verbose")
                .short('v')
                .help("Show diff in message template"),
        )
        .arg(&*argset::HOOK_ARG)
        .arg(
            Arg::new("patchname")
                .help("Name for new patch")
                .value_hint(ValueHint::Other),
        );
    crate::patchedit::add_args(app).arg(&*crate::message::MESSAGE_TEMPLATE_ARG)
}

fn run(matches: &ArgMatches) -> super::Result {
    let repo = git2::Repository::open_from_env()?;
    let branch_name: Option<&str> = None;
    let stack = Stack::from_branch(&repo, branch_name)?;

    let conflicts_okay = false;
    stack.check_repository_state(conflicts_okay)?;
    stack.check_head_top_mismatch()?;

    let mut patchname = if let Some(name) = matches.value_of("patchname") {
        Some(name.parse::<PatchName>()?)
    } else {
        None
    };

    if let Some(ref patchname) = patchname {
        if stack.has_patch(patchname) {
            return Err(Error::PatchAlreadyExists(patchname.clone()));
        }
    }

    let config = repo.config()?;

    let opt_edit = matches.is_present("edit");
    let opt_diff = matches.is_present("diff");
    let verbose =
        matches.is_present("verbose") || config.get_bool("stgit.new.verbose").unwrap_or(false);
    let len_limit: Option<usize> = config
        .get_i32("stgit.namelength")
        .ok()
        .and_then(|n| usize::try_from(n).ok());
    let disallow_patches: Vec<&PatchName> = stack.all_patches().collect();
    let allowed_patches = vec![];

    let head_ref = repo.head()?;
    let tree = head_ref.peel_to_tree()?;
    let parents = vec![head_ref.peel_to_commit()?.id()];

    let (message, must_edit) =
        if let Some(message) = crate::message::get_message_from_args(matches)? {
            let force_edit = opt_edit || opt_diff;
            if force_edit && patchname.is_none() && !message.is_empty() {
                patchname = Some(PatchName::make_unique(
                    &message,
                    len_limit,
                    true,
                    &allowed_patches,
                    &disallow_patches,
                ));
            }
            (message, force_edit)
        } else if let Some(message_template) = crate::message::get_message_template(&repo)? {
            (message_template, true)
        } else {
            (String::new(), true)
        };

    let committer = signature::default_committer(Some(&config))?;
    let autosign = config.get_string("stgit.autosign").ok();
    let message = crate::trailers::add_trailers(message, matches, &committer, autosign.as_deref())?;

    let diff = if must_edit && (verbose || opt_diff) {
        Some(repo.diff_tree_to_workdir(
            Some(&tree),
            Some(DiffOptions::new().enable_fast_untracked_dirs(true)),
        )?)
    } else {
        None
    };

    let patch_desc = PatchDescription {
        patchname,
        author: signature::make_author(Some(&config), matches)?,
        message,
        diff,
    };

    let patch_desc = if must_edit {
        crate::edit::edit_interactive(patch_desc, &config)?
    } else {
        patch_desc
    };

    let message = patch_desc.message;

    let mut cd = CommitData::new(patch_desc.author, committer, message, tree.id(), parents);

    if let Some(template_path) = matches.value_of_os("save-template") {
        std::fs::write(template_path, &cd.message)?;
        return Ok(());
    }

    if !matches.is_present("no-verify") {
        cd = crate::hook::run_commit_msg_hook(&repo, cd, false)?;
    }

    let patchname: PatchName = {
        if let Some(patchname) = patch_desc.patchname {
            if must_edit {
                PatchName::make_unique(
                    patchname.as_ref(),
                    len_limit,
                    false,
                    &allowed_patches,
                    &disallow_patches,
                )
            } else {
                patchname
            }
        } else {
            PatchName::make_unique(
                &cd.message,
                len_limit,
                true, // lowercase
                &allowed_patches,
                &disallow_patches,
            )
        }
    };

    let discard_changes = false;
    let use_index_and_worktree = false;
    stack
        .transaction(
            ConflictMode::Disallow,
            discard_changes,
            use_index_and_worktree,
            |trans| {
                let patch_commit_id = cd.commit(&repo)?;
                trans.push_applied(&patchname, patch_commit_id)?;
                Ok(())
            },
        )
        .execute(&format!("new: {}", patchname))?;
    Ok(())
}
