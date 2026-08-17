type GetterMap<T extends object> = () => T;
type SetterMap<T extends object> = Partial<{ [K in keyof T]: (value: T[K]) => void }>;

/** Keeps controller state live while moving the binding surface out of the controller body. */
export function createBoundController<State extends object, Actions extends object>(
	read: GetterMap<State>,
	write: SetterMap<State>,
	actions: Actions
): State & Actions {
	return new Proxy(actions as State & Actions, {
		get(target, property, receiver) {
			if (property in target) return Reflect.get(target, property, receiver);
			return read()[property as keyof State];
		},
		set(target, property, value, receiver) {
			const setter = write[property as keyof State] as ((next: unknown) => void) | undefined;
			if (setter) {
				setter(value);
				return true;
			}
			return Reflect.set(target, property, value, receiver);
		}
	}) as State & Actions;
}
