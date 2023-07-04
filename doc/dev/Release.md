# How to release Arti

## Tools and notation

We're going to use the following.
Why not upgrade to the latest version before you start?

  * cargo-semver-checks
  * cargo-edit

In the documentation below,
we sometimes use environment variables to indicate
particular versions of Arti.
For example, in the release I just did, I had:

```
LAST_VERSION=1.1.5
THIS_VERSION=1.1.6
```

## Are we ready to release?

Before we can finally release, we need to check a few things
to make sure we aren't going to break our users.

1. Make sure CI is passing.

2. After making sure that the pipeline as a whole has passed,
   look at every part of the pipeline that "passed with warnings".
   Are the warnings what we expect?
   If it's failing, is is it failing for the reasons we anticipated,
   or have new failures crept in?

3. Look at the current list of exceptions in our automated tooling.
   Are they still relevant?
   (There are exceptions in
   `cargo_audit`,
   `check_doc_features`,
   and
   `check_licenses`.)

4. Do we have any open [issues] or [merge requests] tagged "Blocker"?

[issues]: https://gitlab.torproject.org/tpo/core/arti/-/issues/?label_name%5B%5D=Blocker
[merge requests]: https://gitlab.torproject.org/tpo/core/arti/-/merge_requests/?label_name[]=Blocker

5. Does `maint/fixup-features` produce any results?
   If so, fix them.

6. Does `maint/semver-checks` find any issues
   not noted in our semver.md files?
   If so, add them.

Note that you can do these steps _in parallel_ with "preparing for the
release" below.

## Preparing for the release

Note that you can do these steps _in parallel_ with "are we ready to
release?" above.

1. Check for breaking changes to our dependencies.
   In the weeks between releases, I try to run:
   `cargo upgrade --dry-run --compatible=ignore --incompatible=allow`.
   This will tell us if any of our dependencies
   have new versions that will not upgrade automatically.

2. Check for non-breaking changes to our dependencies.
   A day or two before release, I try to run:
   `cargo update`.
   This will replace each of our dependencies in Cargo.lock
   with the latest version.
   (I recommend doing this a bit before the release
   to make sure that we have time
   to deal with any surprising breakage.)

3. Write a changelog.

   I start by running
   `git log --reverse arti-v${LAST_VERSION}..`,
   and writing a short summary of everything I find into
   new sections of the changelog.
   I try to copy the list of sections
   from previous changelogs, to keep them consistent.

   If I have to take a break in the middle,
   or if time will pass between now and the release,
   I add a note like
   `UP TO DATE WITH 726f666c2d2d6d61646520796f75206c6f6f6b21`
   to remind me where I need to start again.

4. Finish the changelog.

   When the changelog is done, pipe it into
   `maint/gen_md_links` to auto-generate markdown links
   to our gitlab repositories.
   Then, fill in the URLs for any links that the script
   couldn't find.

   Run `maint/thanks arti-v${LAST_VERSION}`
   to generate our list of ackowledgments;
   insert this into the changelog.

   Add an acknowledgement for the current sponsor(s).

4. Determine what crates have semver changes.

   We need to sort our crates into the following tiers.
    * No changes were made.
      (No version bump needed)
    * Only non-functional changes were made.
      (Bump the version of the crate, but not the depended-on version.)
    * Functional changes were made, but no APIs were added or broken.
      (Bump patchlevel.)
    * APIs were added.
      (Bump patchlevel if major == 0; else bump minor.)
    * APIs were broken.
      (Bump minor if major == 0; else bump major.)

   You can identify crates that have no changes (by elimination)
   with `./maint/changed_crates ${LAST_VERSION}`.

   To see whether a crate has only non-functional changes,
   you have to use  `git diff`.  Sorry!
   Historically, trivial changes
   are small tweaks in the clippy lints,
   or documentation/comment improvements.

   To determine whether any APIs were added,
   look in the semver.md files.
   (We often forget to add these;
   but fortunately,
   most of our crates currently have major version 0,
   so we don't need to distinguish "functional changes"
   from "new APIs".)

   To determine whether any APIs were broken,
   look in the semver.md files.
   The `cargo semver-checks` tool
   can also identify some of these,
   but its false-negative rate is high.

   You may also want to look over the crate-by-crate diffs.
   This can be quite time-consuming.

## Final preparation for a release.

Wait! Go back and make sure
that you have done everything in the previous sections
before you continue!

1. Finalize the changelog.

   Make sure that the date is correct.
   Make sure that the acknoweldgments and links are correct,
   if they might have gotten stale.

2. Increase all appropriate version numbers.

   To do this, run for each crate with functional changes:
    `cargo set-version --bump {patch|minor|major} -p ${CRATE}`.

   For crates with non-functional changes,
   you need to edit the Cargo.toml files.
   We have [a ticket][#945] to automate this.

[#945]: https://gitlab.torproject.org/tpo/core/arti/-/issues/945

3. Check for side effects from bumping versions!

   Is there a Cargo.lock change you forgot to commit?
   If so, commit it.

   Does a previously unchanged crate
   depend on a crate that got a version bump?
   Now _that_ crate counts as changed,
   and needs a version bump of its own.

   Run `maint/semver-checks` again:
   It should be quiet now that you bumped all the versions.

4. Then make sure that CI passes, again.

## The actual release itself.

1. From lowest-level to highest-level, we have to run cargo publish.  For
   a list of crates from lowest- to highest-level, see the top-level
   Cargo.toml.

   `; for crate in $(./maint/list_crates); do cargo publish -p "$crate"; done`


2. We tag the repository with `arti-v${THIS_VERSION}`

   To do this, run
   `git tag -s "arti-v${THIS_VERSION}`.

   In the tag message, be sure to include the output of
   `./maint/crate_versions`.

   (Note to self: if you find that gpg can't find your yubikey,
   you'll need to run
   `sudo systemctl restart pcscd`
   to set things right.
   I hope that nobody else has this problem.)

## Post-release

1. Remove all of the semver.md files:
   `git rm crates/*/semver.md`.

2. Write a blog post.

3. One the blog post is published,
   update the origin/pages branch to refer to the new version.


<!-- ================================================== -->

# Making a patch release of a crate, eg to fix a semver breakage

If one crate needs to be updated urgently
(eg to fix a build breakage),
we can release it separately as follows.

(The underlying principle is to try to
make precisely the minimal intended change;
and avoid picking up other changes made to Arti crates
since the last release.)

0. File a ticket describing the problem.
   This will be referenced in MRs, release docs, etc.

1. Prepare the changes on a git branch
   starting from the most recent `arti-v...` tag, **not main**.

   (If this happens more than once per Arti release cycle,
   start the branch from the previous crate-specific tag;
   we should perhaps create a `patch-releases` branch
   if this is other than a very rare case.)

2. Changes on this branch should include:
   * The underlying desired change (whether to code or `Cargo.toml`s)
   * Version bump to the crate being released
   * Absolutely minimal necessary changes to `Cargo.lock`.
     (`Cargo.lock` is mostly for CI here.)
   * New stanza in `CHANGELOG.md` describing the point release
     (and cross-referencing to the ticket).

3. Make an MR of this branch.
   State in the MR that the branch is intended for a patch update.
   Obtain a review.

4. If the CI passes, and review is favourable,
   merge the MR and go on to step 5.

5. If the CI failed, you may want to proceed anyway.
   For example, `cargo audit`, or builds with Nightly,
   can rot, without us having done anything.
   If you think you wish to do this:

    1. Review the CI output for the failing command,
       and make a decision about it,
       in consultation with your reviewer.
       If you wish to tolerate this failure:

    2. Create a new git branch `bodge`
       off the patch release branch.

    3. In `.gitlab-ci.yml`,
       prepend the failing shell command with `true X X X`
       (only with the three Xs cuddled together).
       This will cause it to not run, but "succed".
       The `XXXs` will cause the blocking-todos CI job to fail,
       hindering accidental merge of this branch to main.

    4. Make a new MR of this `bodge` branch.
       Say "do not merge" in the title
       and mark it draft.

    5. Await CI on the bodge MR.
       If all that fails is the blocking todos,
       you're good to go.
       Otherwise, go back to sub-step 1
       and review the further failures.

    6. Double check which MR page you are looking at:
       you must be looking at the real patch MR,
       not the CI bodge MR, or some unrelated MR.
       Then, tell gitlab to merge the branch despite CI failure.

6. After the patch branch is merged,
   check it out locally and make releases:

    1. Check that you are on the patch branch, not `main`.

    2. `cargo publish -p the-affected-crate`.

    3. `git tag -s the-affected-crate-vX.Y.Z`
       and push the tag.

7. File any followup tickets and/or do any post-patch cleanup.

8. Consider whether to make a blog post about the patch release.


