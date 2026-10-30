# Claude Code memory and global skills (backup)

A copy of the Tech Lead agent's file memory and the user's global skills, for moving to
another machine. Not read by agents from here.

Restore:

```sh
# memory: the folder name is the project path with '/' -> '-'
mkdir -p ~/.claude/projects/<project-path-dashed>/memory
cp agents/claude-memory/*.md ~/.claude/projects/<project-path-dashed>/memory/
rm ~/.claude/projects/<project-path-dashed>/memory/README.md
# global skills
cp -R agents/claude-skills/* ~/.claude/skills/
```

Not copied: the AWS skills (reinstalled with their plugin) and the skills synced from
claude.ai (they sync again after login).
