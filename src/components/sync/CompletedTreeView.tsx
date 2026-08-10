import { useMemo, useState, useSyncExternalStore } from "react";
import { ChevronDown, ChevronRight, File as FileIcon, Folder, FolderOpen } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { formatBytes } from "@/lib/utils";
import { syncStore, type DirNode } from "@/lib/syncStore";

/** Max tree nodes rendered at once; beyond this we show a truncation hint. */
const MAX_RENDER = 2000;

interface FlatNode {
  key: string;
  name: string;
  type: "dir" | "file";
  depth: number;
  path: string;
  size: number;
  fileCount: number;
  collapsed: boolean;
}

function flattenTree(
  root: DirNode,
  collapsed: Set<string>,
  cap: number
): { nodes: FlatNode[]; truncated: boolean; total: { files: number; size: number } } {
  const nodes: FlatNode[] = [];
  let truncated = false;
  let count = 0;

  function walk(node: DirNode, path: string, depth: number): { files: number; size: number } {
    if (count >= cap) {
      truncated = true;
      return { files: 0, size: 0 };
    }
    const isCollapsed = collapsed.has(path);
    count++;
    const entry: FlatNode = {
      key: path,
      name: node.name || "(根)",
      type: "dir",
      depth,
      path,
      size: 0,
      fileCount: 0,
      collapsed: isCollapsed,
    };
    nodes.push(entry);

    let files = 0;
    let size = 0;
    if (!isCollapsed) {
      for (const d of node.dirs.values()) {
        if (count >= cap) {
          truncated = true;
          break;
        }
        const childPath = path ? `${path}/${d.name}` : d.name;
        const sub = walk(d, childPath, depth + 1);
        files += sub.files;
        size += sub.size;
      }
      if (!truncated) {
        for (const f of node.files.values()) {
          if (count >= cap) {
            truncated = true;
            break;
          }
          count++;
          nodes.push({
            key: path ? `${path}/${f.name}` : f.name,
            name: f.name,
            type: "file",
            depth: depth + 1,
            path: path ? `${path}/${f.name}` : f.name,
            size: f.size,
            fileCount: 0,
            collapsed: false,
          });
          files++;
          size += f.size;
        }
      }
    }
    entry.fileCount = files;
    entry.size = size;
    return { files, size };
  }

  const total = { files: 0, size: 0 };
  for (const d of root.dirs.values()) {
    if (count >= cap) {
      truncated = true;
      break;
    }
    const sub = walk(d, d.name, 0);
    total.files += sub.files;
    total.size += sub.size;
  }
  if (!truncated) {
    for (const f of root.files.values()) {
      if (count >= cap) {
        truncated = true;
        break;
      }
      count++;
      nodes.push({
        key: f.name,
        name: f.name,
        type: "file",
        depth: 1,
        path: f.name,
        size: f.size,
        fileCount: 0,
        collapsed: false,
      });
      total.files++;
      total.size += f.size;
    }
  }
  return { nodes, truncated, total };
}

/**
 * Tree view of successfully transferred files/directories. Data is stored in a
 * compact trie and rendered as a capped, expandable list so it stays responsive
 * for very large sync jobs.
 */
export default function CompletedTreeView() {
  useSyncExternalStore(syncStore.subscribe, syncStore.getSnapshot);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const tree = syncStore.tree;

  const { nodes, truncated, total } = useMemo(
    () => flattenTree(tree.root, collapsed, MAX_RENDER),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [tree.count, collapsed]
  );

  const toggle = (key: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  if (tree.count === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
        <FolderOpen className="size-6 text-muted-foreground/50" />
        <p className="text-sm">暂无已完成文件</p>
      </div>
    );
  }

  return (
    <div className="h-full overflow-auto p-2">
      <div className="mb-2 flex items-center gap-2 px-2 text-xs text-muted-foreground">
        <span>
          已完成 <span className="font-medium text-foreground">{tree.count}</span> 个文件 ·{" "}
          {formatBytes(total.size)}
        </span>
        {truncated && <Badge variant="outline">显示已截断</Badge>}
      </div>

      {nodes.map((n) =>
        n.type === "dir" ? (
          <button
            key={n.key}
            onClick={() => toggle(n.key)}
            style={{ paddingLeft: n.depth * 14 + 8 }}
            className="flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-xs hover:bg-accent"
          >
            {n.collapsed ? (
              <ChevronRight className="size-3.5 shrink-0 text-muted-foreground" />
            ) : (
              <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" />
            )}
            {n.collapsed ? (
              <Folder className="size-3.5 shrink-0 text-amber-500" />
            ) : (
              <FolderOpen className="size-3.5 shrink-0 text-amber-500" />
            )}
            <span className="truncate">{n.name}</span>
            <span className="ml-auto shrink-0 font-mono text-[10px] text-muted-foreground">
              {n.fileCount} 个 · {formatBytes(n.size)}
            </span>
          </button>
        ) : (
          <div
            key={n.key}
            title={n.path}
            style={{ paddingLeft: n.depth * 14 + 8 }}
            className="flex items-center gap-1.5 rounded-md px-2 py-0.5 text-xs"
          >
            <FileIcon className="size-3.5 shrink-0 text-muted-foreground" />
            <span className="truncate">{n.name}</span>
            <span className="ml-auto shrink-0 font-mono text-[10px] text-muted-foreground">
              {formatBytes(n.size)}
            </span>
          </div>
        )
      )}

      {truncated && (
        <div className="py-2 text-center text-[10px] text-muted-foreground">
          还有更多内容未显示，折叠部分目录可查看更深层文件
        </div>
      )}
    </div>
  );
}
