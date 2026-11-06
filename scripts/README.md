# Hematite Extensibility Hooks

Hematite provides hooks for custom Lua (`.lua`) and Rhai (`.rhai`) scripts that run during content indexing.

## Directory Structure
- `scripts/hooks/file/`: Scripts executed for each page entity after Hematite completes its default parsing.
- `scripts/hooks/directory/`: Scripts executed for each directory entity after page hooks complete.

## Execution Order
Scripts within each hook directory are executed in alphabetical order by filename (e.g., `00_session_page_renamer.lua`, `01_stats_enhancer.rhai`).

## Entity & Context Interfaces
- **Globals Available**:
  - `entity`: The page or directory item currently being processed.
  - `context`: Global crawl state containing `all_pages` and `all_directories`, as well as `config`.
  - `config`: Direct access to the parsed `hematite.yaml` configuration (equivalent to `context.config`).
- **Page Hook (`scripts/hooks/file/`)**:
  - `entity` fields: `url_path`, `file_path`, `title`, `parent_path`, `depth`, `is_dir_index`, `frontmatter_json`, `parsed_html`, `links`.
  - `context` fields: `all_pages`, `all_directories`, `config`.
- **Directory Hook (`scripts/hooks/directory/`)**:
  - `entity` fields: `url_path`, `parent_path`, `depth`, `title`, `files`, `subdirectories`, `index_metadata`.
  - `context` fields: `all_pages`, `all_directories`, `config`.
- The returned entity (or modified global `entity`) is saved back to Hematite.

---

## Examples

### 1. Title Formatting (Page Hook)
Transforms page titles by removing numeric prefixes (e.g. `06`), replacing underscores with spaces, capitalizing every word, and trimming excess whitespace. Config parameters (such as a custom site name or prefix option) can also be inspected via `config`.

#### Lua: `scripts/hooks/file/10_clean_title.lua`
```lua
-- Helper function to capitalize each word and clean title
local function clean_title(str)
    -- Remove leading number prefix (e.g., '06', '06_', '06- ', '01.')
    str = str:gsub("^%s*%d+[%s_%.%-]*", "")

    -- Replace underscores with spaces
    str = str:gsub("_", " ")

    -- Trim whitespace
    str = str:match("^%s*(.-)%s*$")

    -- Capitalize each word
    str = str:gsub("(%a)([%w_']*)", function(first, rest)
        return first:upper() .. rest:lower()
    end)

    return str
end

if entity.title and entity.title ~= "" then
    entity.title = clean_title(entity.title)
end

return entity
```

#### Rhai: `scripts/hooks/file/10_clean_title.rhai`
```rust
// Remove numeric prefix (e.g., "06", "06_session_notes")
let title = entity.title;

// Strip leading digits and separators
let cleaned = "";
let skipping_digits = true;
for ch in title {
    if skipping_digits && (ch >= '0' && ch <= '9' || ch == '_' || ch == '-' || ch == '.' || ch == ' ') {
        continue;
    } else {
        skipping_digits = false;
        if ch == '_' {
            cleaned += " ";
        } else {
            cleaned += ch;
        }
    }
}

// Split into words, capitalize each, and trim whitespace
let words = cleaned.split(' ');
let result_words = [];
for word in words {
    word.trim();
    if word.len() > 0 {
        let first = word.sub_string(0, 1).to_upper();
        let rest = if word.len() > 1 { word.sub_string(1).to_lower() } else { "" };
        result_words.push(first + rest);
    }
}

entity.title = result_words.join(" ");
return entity;
```

---

### 2. Unnesting Directories Matching a Pattern (Page & Directory Hooks)
Lifts directories matching a specific pattern (e.g., folders starting with `archive_` or configured via `config.unnest_pattern`) up by one level in both URL hierarchy and sidebar navigation.

#### Lua Page Hook: `scripts/hooks/file/05_unnest_pages.lua`
```lua
-- Match directory pattern to unnest (e.g. from config or default pattern)
local pattern = config.unnest_pattern or "archive_"

-- If url_path contains the matched directory segment, promote one level up
-- e.g. "/campaigns/archive_2024/session-01" -> "/campaigns/session-01"
local match_str = "/" .. pattern .. "[^/]+"
if entity.url_path:find(match_str) then
    entity.url_path = entity.url_path:gsub(match_str, "")
    
    -- Recalculate parent_path and depth
    if entity.url_path == "/" then
        entity.parent_path = ""
        entity.depth = 0
    else
        local last_slash = entity.url_path:match("^(.*)/[^/]*$")
        entity.parent_path = (last_slash == "") and "/" or last_slash
        local _, slash_count = entity.url_path:gsub("/", "")
        entity.depth = slash_count
    end
end

return entity
```

#### Lua Directory Hook: `scripts/hooks/directory/05_unnest_directory.lua`
```lua
-- Keep directory sidebar navigation aligned with unnested page URLs
local pattern = config.unnest_pattern or "archive_"
local match_str = "/" .. pattern .. "[^/]+"

if entity.url_path:find(match_str) then
    entity.url_path = entity.url_path:gsub(match_str, "")
    
    if entity.url_path == "/" then
        entity.parent_path = ""
        entity.depth = 0
    else
        local last_slash = entity.url_path:match("^(.*)/[^/]*$")
        entity.parent_path = (last_slash == "") and "/" or last_slash
        local _, slash_count = entity.url_path:gsub("/", "")
        entity.depth = slash_count
    end
end

return entity
```

#### Rhai Directory Hook: `scripts/hooks/directory/05_unnest_directory.rhai`
```rust
// Check if current directory path matches unnest prefix
let pattern = "/archive_";

if entity.url_path.contains(pattern) {
    let parts = entity.url_path.split('/');
    let new_parts = [];
    
    for part in parts {
        if !part.starts_with("archive_") && part.len() > 0 {
            new_parts.push(part);
        }
    }
    
    entity.url_path = "/" + new_parts.join("/");
    
    if entity.url_path == "/" {
        entity.parent_path = "";
        entity.depth = 0;
    } else {
        let last_idx = entity.url_path.index_of('/');
        let parent = "";
        let slash_count = 0;
        for ch in entity.url_path {
            if ch == '/' { slash_count += 1; }
        }
        
        let segs = entity.url_path.split('/');
        if segs.len() <= 2 {
            entity.parent_path = "/";
        } else {
            segs.pop();
            entity.parent_path = segs.join("/");
        }
        entity.depth = slash_count;
    }
}

return entity;
```
