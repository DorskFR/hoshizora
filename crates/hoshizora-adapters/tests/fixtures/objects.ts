import { api } from './api';
import { endpoints } from './endpoints';

export const keys = {
  session: (id: string) => ['session', id],
  all() {
    return ['sessions'];
  },
};

export function useSessionActions() {
  const invalidate = () => api.get('/sessions');
  return {
    spawn: (body: unknown, files: File[] = []) => endpoints.spawn(body, files),
    archive(id: string) {
      return api.post(`/sessions/${id}/archive`);
    },
  };
}

export class SpawnForm {
  readonly actions = useSessionActions();
  private store: Store;
  files: File[] = [];
  readonly client = new Client('x');

  async submit() {
    const body = this.buildBody();
    await this.actions.spawn(body, this.files);
    render(body, this.files, show);
  }

  buildBody() {
    return {};
  }
}

function show() {}
