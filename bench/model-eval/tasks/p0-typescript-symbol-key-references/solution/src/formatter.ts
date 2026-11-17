export interface Formatter {
  renderLabel(value: string): string;
}

export const formatter: Formatter = {
  renderLabel(value: string): string {
    return value.trim().toUpperCase();
  },
};
