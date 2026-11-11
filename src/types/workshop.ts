export interface CollectionRef {
  id: string;
  title: string;
  item_count: number;
}

export type RawTag = string | { tag?: string; category?: string };

export interface WorkshopItem {
  pubfileid: string;
  title: string;
  preview_url: string;
  author: string;
  author_url: string;
  description: string;
  file_size: string;
  posted_date: string;
  updated_date: string;
  tags: RawTag[];
  rating_star_file: string;
  num_ratings: string;
  is_collection: boolean;
  collections?: CollectionRef[];
}

export interface WorkshopPage {
  items: WorkshopItem[];
  total_pages: number;
  total_items: number;
  current_page: number;
}

export interface CollectionInfo {
  rating_star_file?: string;
  num_ratings?: string;
  item_count?: number;
  unique_visitors?: string;
  subscribers?: string;
  favorited?: string;
  total_favorited?: string;
  posted_date?: string;
  updated_date?: string;
  [key: string]: string | number | undefined;
}

export interface CollectionContents {
  collection_id: string;
  title: string;
  description: string;
  preview_url: string;
  author: string;
  author_url: string;
  items: WorkshopItem[];
  related_collections?: WorkshopItem[];
  info?: CollectionInfo;
}

/** Per-item metadata returned by the backend metadata cache (tag/rating/date). */
export interface InstalledMetadata {
  tags?: unknown[];
  rating_star_file?: string;
  posted_date?: string;
  updated_date?: string;
  author?: string;
}

/** Where the wallpaper was found on disk. */
export type InstalledSource = "myprojects" | "defaultprojects" | "workshop";

export interface InstalledWallpaper {
  pubfileid: string;
  folder: string;
  project_json_path: string;
  has_pkg: boolean;
  title: string;
  preview: string;
  description: string;
  file_type: string;
  tags: string[];
  size_bytes: number;
  installed_ts: number;
  /**
   * myprojects     — downloaded by WEave itself
   * defaultprojects — one of Wallpaper Engine's bundled samples
   * workshop       — a Steam Workshop subscription (Steam owns the folder)
   */
  source: InstalledSource;
}


