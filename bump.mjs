#!/usr/bin/env node

/**
 * Usage:
 *   node bump.mjs          -> Auto-determines next SemVer based on git commit history
 *   node bump.mjs 4.12.0   -> Manually specifies the target version
 *
 * Updates version strings across all locations, generates local Detailed Changelog,
 * updates CHANGELOG.md, pauses for user confirmation/editing, then stages files,
 * creates a git commit, tags the release, and pushes to remote.
 */

import { readFileSync, writeFileSync, existsSync } from "fs";
import { execSync } from "child_process";
import readline from "readline";

const REPO_URL = "https://github.com/psyattack/WEave";
const authorCache = new Map();

function waitForEnter(query) {
  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
  });
  return new Promise((resolve) =>
    rl.question(query, () => {
      rl.close();
      resolve();
    }),
  );
}

async function resolveAuthor(hash, authorName, authorEmail) {
  const cacheKey = `${authorName}<${authorEmail}>`;
  if (authorCache.has(cacheKey)) {
    return authorCache.get(cacheKey);
  }

  // 1. Check GitHub noreply email
  if (authorEmail && authorEmail.includes("@users.noreply.github.com")) {
    const match = authorEmail.match(/^(?:\d+\+)?([^@]+)@users\.noreply\.github\.com$/);
    if (match && match[1]) {
      const handle = `@${match[1]}`;
      authorCache.set(cacheKey, handle);
      return handle;
    }
  }

  // 2. Query GitHub API for commit author login
  try {
    const repoMatch = REPO_URL.match(/github\.com\/([^/]+\/[^/]+)/);
    if (repoMatch && repoMatch[1]) {
      const repo = repoMatch[1];
      const res = await fetch(`https://api.github.com/repos/${repo}/commits/${hash}`, {
        headers: { "User-Agent": "Node-WEave-Release" },
      });
      if (res.ok) {
        const data = await res.json();
        if (data.author && data.author.login) {
          const handle = `@${data.author.login}`;
          authorCache.set(cacheKey, handle);
          return handle;
        }
      }
    }
  } catch {
    // Offline or API error fallback
  }

  // 3. Fallback: plain author name without @
  const fallback = authorName ? authorName.trim() : "";
  authorCache.set(cacheKey, fallback);
  return fallback;
}

async function formatDetailedChangelog(rawEntries) {
  // Strict Keep a Changelog categories
  const categories = {
    feat: "Added",
    feature: "Added",
    fix: "Fixed",
    bugfix: "Fixed",
    perf: "Changed",
    refactor: "Changed",
    style: "Changed",
    chore: "Changed",
    build: "Changed",
    ci: "Changed",
    docs: "Changed",
    deprecate: "Deprecated",
    deprecated: "Deprecated",
    remove: "Removed",
    revert: "Removed",
    sec: "Security",
    security: "Security",
  };

  const grouped = {};

  for (const entry of rawEntries) {
    const lines = entry.trim().split(/\r?\n/);
    if (lines.length < 4) continue;

    const hash = lines[0].trim();
    const shortHash = hash.slice(0, 7);
    const subject = lines[1].trim();
    const authorName = lines[2].trim();
    const authorEmail = lines[3].trim();

    if (!subject) continue;

    // Comprehensive filter for technical, administrative, merge, revert, and release commits
    const lowerSubj = subject.toLowerCase();
    if (
      lowerSubj.startsWith("merge ") ||
      lowerSubj.startsWith("merge pull request") ||
      lowerSubj.startsWith("merge branch") ||
      lowerSubj.startsWith("merge remote-tracking branch") ||
      lowerSubj.startsWith("merge tag") ||
      lowerSubj.startsWith("fixup!") ||
      lowerSubj.startsWith("squash!") ||
      lowerSubj.startsWith("wip:") ||
      lowerSubj === "wip" ||
      lowerSubj === "initial commit" ||
      lowerSubj.startsWith("chore(release)") ||
      lowerSubj.startsWith("bump version to")
    ) {
      continue;
    }

    // Parse conventional commit: type(scope): message
    const match = subject.match(/^(\w+)(?:\(([^)]+)\))?:\s*(.+)$/);
    if (!match) continue;

    const rawType = match[1].toLowerCase();
    const scope = match[2] || null;
    let title = match[3];
    const catName = categories[rawType] || "Changed";
    if (!grouped[catName]) {
      grouped[catName] = [];
    }

    const authorStr = await resolveAuthor(hash, authorName, authorEmail);

    // Extract PR number if present e.g. (#12) or #12
    const prMatch = title.match(/^(.*?)\s*\((?:#|PR\s*)(\d+)\)$/i) || title.match(/^(.*?)\s*#(\d+)$/i);
    let linkStr;
    if (prMatch) {
      title = prMatch[1].trim();
      const prNum = prMatch[2];
      linkStr = `#${prNum}`;
    } else {
      linkStr = shortHash;
    }

    const scopeStr = scope ? `**${scope}:** ` : "";
    const authorPart = authorStr ? `by ${authorStr} in ` : "in ";
    const lineItem = `- ${scopeStr}${title} (${authorPart}${linkStr})`;

    grouped[catName].push(lineItem);
  }

  const categoryOrder = ["Added", "Changed", "Deprecated", "Removed", "Fixed", "Security"];
  const categoryKeys = categoryOrder.filter((cat) => grouped[cat] && grouped[cat].length > 0);

  if (categoryKeys.length === 0) {
    return "";
  }

  let markdown = "<details><summary>Detailed Changelog</summary>\n\n";
  for (let i = 0; i < categoryKeys.length; i++) {
    const cat = categoryKeys[i];
    markdown += `### ${cat}\n`;
    for (const item of grouped[cat]) {
      markdown += `${item}\n`;
    }
    if (i < categoryKeys.length - 1) {
      markdown += "\n";
    }
  }
  markdown += "\n</details>";

  return markdown;
}

async function main() {
  // 1. Read current version from package.json
  const pkg = JSON.parse(readFileSync("package.json", "utf-8"));
  const currentVersion = pkg.version;

  let newVersion = process.argv[2];

  let prevTag = "";
  try {
    prevTag = execSync("git describe --tags --abbrev=0").toString().trim();
  } catch {
    // No tags found
  }

  const range = prevTag ? `${prevTag}..HEAD` : "HEAD";
  let rawLog;
  try {
    rawLog = execSync(
      `git log ${range} --pretty=format:"%H%n%s%n%an%n%ae%n-END-"`,
    ).toString();
  } catch {
    rawLog = "";
  }

  const entries = rawLog.split("-END-").filter((e) => e.trim());

  if (!newVersion) {
    // Auto-determine version if not provided manually
    let bumpType = "patch"; // default

    for (const entry of entries) {
      const subject = entry.trim().split(/\r?\n/)[1] || "";
      if (/BREAKING CHANGE|^\w+(\(.*\))?!:/i.test(subject)) {
        bumpType = "major";
        break;
      } else if (/^feat(\(.*\))?:/i.test(subject)) {
        bumpType = "minor";
      }
    }

    const parts = currentVersion.split(".").map(Number);
    let [major, minor, patch] = parts;

    if (bumpType === "major") {
      major += 1;
      minor = 0;
      patch = 0;
    } else if (bumpType === "minor") {
      minor += 1;
      patch = 0;
    } else {
      patch += 1;
    }

    newVersion = `${major}.${minor}.${patch}`;
    console.log(
      `Auto-determined next version: ${newVersion} (${bumpType} release based on git history)`,
    );
  } else {
    if (!/^\d+\.\d+\.\d+/.test(newVersion)) {
      console.error(
        "ERROR: Invalid semver format provided. Example: node bump.mjs 4.12.0",
      );
      process.exit(1);
    }
    console.log(`Using manually specified version: ${newVersion}`);
  }

  // 2. Generate local Detailed Changelog
  console.log(`\nGenerating Detailed Changelog...`);
  const detailedChangelog = await formatDetailedChangelog(entries);

  // 3. Update version strings across files
  const files = [
    {
      path: "package.json",
      pattern: new RegExp('("version"\\s*:\\s*")\\d+\\.\\d+\\.\\d+(")'),
      replace: `$1${newVersion}$2`,
    },
    {
      path: "src-tauri/Cargo.toml",
      pattern: new RegExp('^(version\\s*=\\s*")\\d+\\.\\d+\\.\\d+(")', "m"),
      replace: `$1${newVersion}$2`,
    },
    {
      path: "src-tauri/tauri.conf.json",
      pattern: new RegExp('("version"\\s*:\\s*")\\d+\\.\\d+\\.\\d+(")'),
      replace: `$1${newVersion}$2`,
    },
    {
      path: "src-tauri/src/core/constants.rs",
      pattern: new RegExp(
        '^(pub const APP_VERSION: &str = ")\\d+\\.\\d+\\.\\d+(")',
        "m",
      ),
      replace: `$1${newVersion}$2`,
    },
  ];

  console.log(`\nBumping version from ${currentVersion} to ${newVersion}...\n`);

  for (const file of files) {
    const content = readFileSync(file.path, "utf-8");

    if (!file.pattern.test(content)) {
      console.error(`ERROR: Could not find version pattern in ${file.path}`);
      process.exit(1);
    }

    const updated = content.replace(file.pattern, file.replace);
    writeFileSync(file.path, updated, "utf-8");

    const oldMatch = content.match(file.pattern);
    const oldVersion = oldMatch
      ? (oldMatch[0].match(/\d+\.\d+\.\d+/)?.[0] ?? "???")
      : "???";

    console.log(`  ${file.path}: ${oldVersion} → ${newVersion}`);
  }

  // 4. Always update or create CHANGELOG.md with Detailed Changelog
  const today = new Date().toISOString().split("T")[0];
  const compareUrl = prevTag ? `${REPO_URL}/compare/${prevTag}...v${newVersion}` : null;
  const versionLink = compareUrl ? `[${newVersion}](${compareUrl})` : `[${newVersion}]`;
  const header = `## ${versionLink} - ${today}`;

  let changelogContent = existsSync("CHANGELOG.md")
    ? readFileSync("CHANGELOG.md", "utf-8")
    : "# Changelog\n\n";

  if (changelogContent.includes(`## [${newVersion}]`)) {
    // If version section exists (e.g. user manually wrote human notes)
    if (detailedChangelog && !changelogContent.includes("Detailed Changelog")) {
      changelogContent = changelogContent.replace(
        new RegExp(`(##\\s*\\[${newVersion}\\].*?)(\\r?\\n(?=##\\s*\\[|\\z))`, "s"),
        `$1\n\n${detailedChangelog.trim()}\n`,
      );
      writeFileSync("CHANGELOG.md", changelogContent, "utf-8");
      console.log(`  CHANGELOG.md: Appended Detailed Changelog to ## [${newVersion}]`);
    }
  } else {
    // Create new version section with Detailed Changelog
    const newSection = `${header}\n\n${detailedChangelog ? detailedChangelog.trim() + "\n" : ""}`;
    if (/# Changelog\r?\n/.test(changelogContent)) {
      changelogContent = changelogContent.replace(
        /# Changelog\r?\n/,
        `# Changelog\n\n${newSection}`,
      );
    } else {
      changelogContent = `${newSection}${changelogContent}`;
    }
    writeFileSync("CHANGELOG.md", changelogContent, "utf-8");
    console.log(`  CHANGELOG.md: Added section ## [${newVersion}] with Detailed Changelog`);
  }

  console.log(`\nSyncing package-lock.json...`);
  execSync("npm install --package-lock-only --ignore-scripts", {
    stdio: "inherit",
  });

  console.log(`\nSyncing Cargo.lock...`);
  execSync("cargo check", { cwd: "src-tauri", stdio: "inherit" });

  console.log(`\n================================================================`);
  console.log(`CHANGELOG.md has been created/updated with version ## [${newVersion}] and Detailed Changelog.`);
  console.log(`You can now inspect or edit CHANGELOG.md.`);
  console.log(`================================================================\n`);

  await waitForEnter("Press ENTER when ready to commit, tag, and push to remote... ");

  console.log(`\nStaging files...`);
  execSync(
    "git add package.json package-lock.json src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/tauri.conf.json src-tauri/src/core/constants.rs CHANGELOG.md",
    { stdio: "inherit" },
  );

  const commitMsg = `chore(release): bump version to ${newVersion}`;
  console.log(`\nCreating commit: "${commitMsg}"...`);
  execSync(`git commit -m "${commitMsg}"`, { stdio: "inherit" });

  const tagName = `v${newVersion}`;
  console.log(`\nCreating git tag: ${tagName}...`);
  execSync(`git tag ${tagName} -m "Release ${tagName}"`, { stdio: "inherit" });

  console.log(`\nPushing commit and tag to remote...`);
  execSync(`git push origin HEAD ${tagName}`, { stdio: "inherit" });

  console.log(
    `\nSuccessfully bumped to ${newVersion}, tagged ${tagName}, and pushed to remote!`,
  );
}

main().catch(console.error);
