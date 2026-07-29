# Branch protection, as code

GitHub does not read these files. They are the checked-in source of truth for
rulesets that are applied through the API, so the protection on `main` is
reviewable and re-appliable rather than a setting somebody remembers changing.

## Apply

```sh
gh api repos/Section9Labs/rupu/rulesets --method POST --input .github/rulesets/main.json
```

To update an existing ruleset, get its id from `gh api
repos/Section9Labs/rupu/rulesets` and `PUT` to
`repos/Section9Labs/rupu/rulesets/<id>` with the same file.

## What `main.json` does

| Rule | Effect |
|---|---|
| `deletion` | `main` cannot be deleted |
| `non_fast_forward` | no force-push; history cannot be rewritten |
| `pull_request` | every change reaches `main` through a PR |
| `required_status_checks` | `linux musl (build + test)` and `community package definitions` must pass before merge |

## Two deliberate choices

**`required_approving_review_count: 0`.** GitHub does not let you approve your
own pull request, so requiring one approval would lock a solo maintainer out of
their own repository. The PR requirement still holds — changes cannot land as a
direct push — but nobody has to sign off. Raise this to `1` the moment there is
a second maintainer.

**The bypass actor is a deploy key, and that was forced.** The release
workflow's `community` job pushes the regenerated `flake.nix`,
`packaging/aur/PKGBUILD`, and `packaging/homebrew/rupu.rb` straight to `main` —
a direct push, which the `pull_request` rule blocks. Something has to bypass.

The obvious candidate does not work. Naming the first-party GitHub Actions
integration as a bypass actor is rejected:

```
HTTP 422: Actor GitHub Actions integration must be part of the ruleset source
          or owner organization
```

Repository rulesets can only bypass on actors the repository itself owns, and
`DeployKey` is one. So the `community` job pushes over SSH with the
`MAIN_PUSH_KEY` deploy key.

That is a better place to land than a PAT. The key can push to this one
repository and do nothing else — it cannot read secrets, change settings, or
reach any other repository — whereas a PAT would carry the whole of the issuing
user's access. It is also the only deploy key on `rupu`, so the `DeployKey`
bypass grants exactly that one job.

## If the POST returns 422 on the bypass actor

`actor_id` for `DeployKey` is documented as `null`. Some API versions want `1`
instead. Try `null` first; if it is rejected, change it to `1`.

## Emergency direct push

There is deliberately no admin bypass, so the protection is real rather than
advisory. To push directly, set `"enforcement": "disabled"` via `PUT`, push,
then set it back to `"active"`. Making that two visible API calls is the point.
