# Rask Blog

This is the Jekyll-powered blog for Rask, deployed alongside the main mdBook documentation.

## Writing Posts

Create new posts in `_posts/` using the naming convention:

```
YYYY-MM-DD-title-slug.md
```

### Post Template

```markdown
---
layout: post
title: "Your Post Title"
date: 2026-02-07 12:00:00 +0100
categories: [announcement, development, design]
---

Your content here...
```

### Categories

Common categories:
- `announcement` - Project updates, releases
- `development` - Implementation details, progress
- `design` - Language design decisions
- `tutorial` - How-to guides

## Local Development

```bash
cd docs/blog
bundle install
bundle exec jekyll serve
```

Visit `http://localhost:4000/rask/blog/`

## Structure

- `_posts/` - Blog posts (date-named markdown files)
- `_config.yml` - Jekyll configuration
- `index.md` - Blog home page
- `about.md` - About page

## Theme

Uses the default [Minima](https://github.com/jekyll/minima) theme. To customize, override layouts in `_layouts/` or add custom CSS.
