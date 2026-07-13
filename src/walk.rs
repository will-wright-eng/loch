use anyhow::{Context, Result};
use gix::ObjectId;

/// Resolve `refname` and return the first-parent chain as oldest-first commit ids.
///
/// "Oldest-first" is the reversed tip→root first-parent chain
/// (`git log --first-parent --reverse`); commit timestamps play no part in ordering.
pub fn first_parent_oldest_first(repo: &gix::Repository, refname: &str) -> Result<Vec<ObjectId>> {
    let tip = repo
        .rev_parse_single(refname)
        .with_context(|| format!("cannot resolve '{refname}' (empty repository or unborn ref?)"))?;
    let commit_id = tip
        .object()
        .context("failed to read the object the ref points at")?
        .peel_to_kind(gix::object::Kind::Commit)
        .with_context(|| format!("'{refname}' does not point at a commit"))?
        .id;

    let mut ids = Vec::new();
    for info in repo
        .rev_walk([commit_id])
        .first_parent_only()
        .all()
        .context("failed to start the revision walk")?
    {
        ids.push(info.context("revision walk failed")?.id);
    }
    ids.reverse();
    Ok(ids)
}
