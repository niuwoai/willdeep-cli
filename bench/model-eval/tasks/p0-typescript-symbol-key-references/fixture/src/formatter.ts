export interface Formatter {
  formatLabel(value: string): string;
}

export const formatter: Formatter = {
  formatLabel(value: string): string {
    return value.trim().toUpperCase();
  },
};
