export interface Server { id: string; label: string; baseUrl: string; token: string }
export const SERVER_COLORS = ['#7c6af7', '#3ba55d', '#e0a23b', '#e0563b', '#3b9fe0', '#c43be0'];
export function serverColor(id: string): string {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
  return SERVER_COLORS[h % SERVER_COLORS.length];
}
