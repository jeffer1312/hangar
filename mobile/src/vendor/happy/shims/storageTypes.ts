import { z } from 'zod';

export type Metadata = {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  [k: string]: any;
  path?: string;
  host?: string;
  client?: { id: string; name: string; version: string };
  provider?: { id: string; kind: string; name: string };
  flavor?: string | null;
};

export const TodoItemSchema = z.object({
  content: z.string(),
  status: z.enum(['pending', 'in_progress', 'completed']),
  activeForm: z.string().optional(),
});
export const TodoItemsSchema = z.array(TodoItemSchema);
