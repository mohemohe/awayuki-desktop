declare module "full-emoji-list" {
  export type FullEmojiListItem = {
    CodePointsHex: string[] | null;
    Status: string | null;
    Emoji: string | null;
    Version: string | null;
    Name: string | null;
    Group: string | null;
    SubGroup: string | null;
  };

  const fullEmojiList: FullEmojiListItem[];
  export default fullEmojiList;
}
