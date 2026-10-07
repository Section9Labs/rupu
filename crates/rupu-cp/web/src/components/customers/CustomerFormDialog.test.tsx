// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { api, ApiError, type CustomerDto } from '../../lib/api';
import * as scope from '../../lib/customerScope';
import { CustomerFormDialog } from './CustomerFormDialog';

const DTO: CustomerDto = {
  slug: 'acme-corp',
  name: 'Acme Corp',
  notes: null,
  contact: null,
  color: null,
  tint: { light: '#111111', dark: '#eeeeee' },
  archived: false,
  created_at: '2026-10-01T00:00:00Z',
};

let reload: ReturnType<typeof vi.fn>;
beforeEach(() => {
  reload = vi.fn();
  vi.spyOn(scope, 'useCustomerScope').mockReturnValue({ reload } as unknown as scope.CustomerScopeValue);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderCreate(onSaved = vi.fn(), onClose = vi.fn()) {
  render(<CustomerFormDialog mode="create" onSaved={onSaved} onClose={onClose} />);
  return { onSaved, onClose };
}
const name = () => screen.getByLabelText(/^Name/) as HTMLInputElement;
const slug = () => screen.getByLabelText(/^Slug/) as HTMLInputElement;
const submit = () => screen.getByRole('button', { name: 'Create customer' });

describe('CustomerFormDialog', () => {
  it('auto-suggests the slug from the name until the slug is edited', () => {
    renderCreate();
    fireEvent.change(name(), { target: { value: '  Acme & Sons, Inc.  ' } });
    expect(slug().value).toBe('acme-sons-inc');
    fireEvent.change(slug(), { target: { value: 'mine' } });
    fireEvent.change(name(), { target: { value: 'Other' } });
    expect(slug().value).toBe('mine');
  });

  it('blocks an invalid slug inline and keeps submit disabled', () => {
    renderCreate();
    fireEvent.change(name(), { target: { value: 'Acme' } });
    fireEvent.change(slug(), { target: { value: '-bad slug' } });
    expect(screen.getByText(/lowercase letters, digits and dashes/i)).toBeInTheDocument();
    expect(submit()).toBeDisabled();
  });

  it('rejects the reserved slug none', () => {
    renderCreate();
    fireEvent.change(name(), { target: { value: 'None' } });
    expect(slug().value).toBe('none');
    expect(screen.getByText(/reserved/i)).toBeInTheDocument();
    expect(submit()).toBeDisabled();
  });

  it('requires a name', () => {
    renderCreate();
    fireEvent.change(slug(), { target: { value: 'acme' } });
    expect(submit()).toBeDisabled();
  });

  it('rejects a malformed color', () => {
    renderCreate();
    fireEvent.change(name(), { target: { value: 'Acme' } });
    fireEvent.change(screen.getByLabelText(/^Color/), { target: { value: 'red' } });
    expect(screen.getByText(/#rrggbb/)).toBeInTheDocument();
    expect(submit()).toBeDisabled();
  });

  it('creates, reloads the scope list and reports the saved customer', async () => {
    const create = vi.spyOn(api, 'createCustomer').mockResolvedValue(DTO);
    const { onSaved } = renderCreate();
    fireEvent.change(name(), { target: { value: 'Acme Corp' } });
    fireEvent.change(screen.getByLabelText(/^Contact/), { target: { value: 'a@acme.test' } });
    fireEvent.click(submit());
    await waitFor(() => expect(onSaved).toHaveBeenCalledWith(DTO));
    expect(create).toHaveBeenCalledWith({ slug: 'acme-corp', name: 'Acme Corp', contact: 'a@acme.test' });
    expect(reload).toHaveBeenCalled();
  });

  it('shows a 409 inline on the slug', async () => {
    vi.spyOn(api, 'createCustomer').mockRejectedValue(
      new ApiError(409, 'x', '{"error":"customer \\"acme\\" already exists"}'),
    );
    const { onSaved } = renderCreate();
    fireEvent.change(name(), { target: { value: 'Acme' } });
    fireEvent.click(submit());
    expect(await screen.findByText(/already exists/)).toBeInTheDocument();
    expect(slug()).toHaveAttribute('aria-invalid', 'true');
    expect(onSaved).not.toHaveBeenCalled();
    expect(reload).not.toHaveBeenCalled();
  });

  it('shows another 400 inline in the dialog', async () => {
    vi.spyOn(api, 'createCustomer').mockRejectedValue(new ApiError(400, 'x', '{"error":"name must not be empty"}'));
    renderCreate();
    fireEvent.change(name(), { target: { value: 'Acme' } });
    fireEvent.click(submit());
    expect(await screen.findByRole('alert')).toHaveTextContent('name must not be empty');
  });

  it('edit mode has no slug field and patches, clearing emptied fields', async () => {
    const update = vi.spyOn(api, 'updateCustomer').mockResolvedValue(DTO);
    const onSaved = vi.fn();
    render(
      <CustomerFormDialog
        mode="edit"
        initial={{ ...DTO, notes: 'old notes', color: '#336699' }}
        onSaved={onSaved}
        onClose={vi.fn()}
      />,
    );
    expect(screen.queryByLabelText(/^Slug/)).toBeNull();
    fireEvent.change(screen.getByLabelText(/^Notes/), { target: { value: '' } });
    fireEvent.change(screen.getByLabelText(/^Color/), { target: { value: '' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(onSaved).toHaveBeenCalled());
    expect(update).toHaveBeenCalledWith('acme-corp', { name: 'Acme Corp', notes: '', contact: '', color: '' });
    expect(reload).toHaveBeenCalled();
  });

  it('Escape and Cancel close a pristine form', () => {
    const { onClose } = renderCreate();
    fireEvent.keyDown(document, { key: 'Escape' });
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it('a dirty form asks "Discard changes?" before Escape / overlay / Cancel close it', () => {
    const { onClose } = renderCreate();
    fireEvent.change(name(), { target: { value: 'Acme' } });
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByText('Discard changes?')).toBeInTheDocument();
    // Escape on the prompt keeps editing.
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByText('Discard changes?')).toBeNull();
    expect(name().value).toBe('Acme');
    // Overlay click and Cancel ask again.
    fireEvent.mouseDown(screen.getByTestId('customer-form-overlay'));
    expect(screen.getByText('Discard changes?')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Keep editing' }));
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Discard' }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('ignores Escape and overlay clicks while saving', async () => {
    let resolve!: (d: CustomerDto) => void;
    vi.spyOn(api, 'createCustomer').mockReturnValue(new Promise((r) => (resolve = r)));
    const { onClose, onSaved } = renderCreate();
    fireEvent.change(name(), { target: { value: 'Acme' } });
    fireEvent.click(submit());
    await waitFor(() => expect(submit()).toBeDisabled());
    fireEvent.keyDown(document, { key: 'Escape' });
    fireEvent.mouseDown(screen.getByTestId('customer-form-overlay'));
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.queryByText('Discard changes?')).toBeNull();
    resolve(DTO);
    await waitFor(() => expect(onSaved).toHaveBeenCalled());
  });

  it('create with an empty color shows a neutral dot and "derived from the slug"', () => {
    const { container } = render(
      <CustomerFormDialog mode="create" onSaved={vi.fn()} onClose={vi.fn()} />,
    );
    expect(container.querySelector('[data-neutral-dot]')).not.toBeNull();
    expect(container.querySelector('[data-customer-dot]')).toBeNull();
    expect(screen.getByText(/derived from the slug/)).toBeInTheDocument();
  });
});
