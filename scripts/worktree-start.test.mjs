// SPDX-License-Identifier: Apache-2.0
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { slugifyBranch, validateBranchName, worktreeDirFor } from "./worktree-start.mjs";

describe("worktree-start branch naming", () => {
  it("slugs a scoped branch into a directory-safe name", () => {
    assert.equal(slugifyBranch("fix/my-change"), "fix-my-change");
    assert.equal(slugifyBranch("feat/foo_bar.baz"), "feat-foo-bar-baz");
    assert.equal(slugifyBranch("fix/use-case-2"), "fix-use-case-2");
  });

  it("collapses and trims separators", () => {
    assert.equal(slugifyBranch("fix//double"), "fix-double");
    assert.equal(slugifyBranch("--fix/leading--"), "fix-leading");
  });

  it("accepts conventional scoped branch names", () => {
    assert.equal(validateBranchName("fix/my-change"), null);
    assert.equal(validateBranchName("feat/foo-bar_2"), null);
    assert.equal(validateBranchName("docs/worktree"), null);
  });

  it("rejects missing, reserved, and malformed names", () => {
    assert.match(validateBranchName(""), /missing branch name/);
    assert.match(validateBranchName("main"), /reserved/);
    assert.match(validateBranchName("origin"), /reserved/);
    assert.match(validateBranchName("Fix/My-Change"), /invalid branch name/);
    assert.match(validateBranchName("fix/with space"), /invalid branch name/);
    assert.match(validateBranchName("fix/"), /invalid branch name/);
    assert.match(validateBranchName("/leading"), /invalid branch name/);
  });

  it("places worktrees under .claude/worktrees with the slug", () => {
    const dir = worktreeDirFor("/repo", "fix/my-change");
    assert.equal(dir, path.join("/repo", ".claude", "worktrees", "fix-my-change"));
  });
});
