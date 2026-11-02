<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Releasing the runner

The runner is a file attached to a GitHub Release, and the action fetches it from there. It is not on npm, by the maintainer's decision: `"private": true` in
`runner/package.json` stops an accidental `npm publish`. No account or token other than GitHub's is involved.

## A release

1. Set the same version in `runner/package.json` and in the `package` default of `action/action.yml`. A test checks that they agree.
2. Merge, then push the tag from an up-to-date `main`:

       git tag v0.1.0
       git push origin v0.1.0

3. `.github/workflows/release.yml` checks that the tag matches, runs the tests, builds the package and attaches it to a new GitHub Release.

## The action's tag

`uses: .../action@v1` follows the tag `v1`. Move it once a release is out and checked:

    git tag -f v1 v0.1.0
    git push -f origin v1
