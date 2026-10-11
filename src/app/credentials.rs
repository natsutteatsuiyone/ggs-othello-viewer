use gpui_kit::{App, Task};

type Credential = Option<(String, Vec<u8>)>;
#[cfg(test)]
type Store = std::collections::BTreeMap<String, (String, Vec<u8>)>;

#[derive(Clone, Default)]
pub(super) struct Credentials {
    #[cfg(test)]
    pub entries: std::rc::Rc<std::cell::RefCell<Store>>,
    #[cfg(test)]
    pub delayed_read: std::rc::Rc<std::cell::RefCell<Option<async_channel::Receiver<Credential>>>>,
    #[cfg(test)]
    pub delayed_write: std::rc::Rc<std::cell::RefCell<Option<async_channel::Receiver<()>>>>,
    #[cfg(test)]
    pub fail: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Credentials {
    pub fn read(&self, key: &str, cx: &App) -> Task<Result<Credential, ()>> {
        #[cfg(not(test))]
        {
            let task = cx.read_credentials(key);
            cx.background_executor().spawn(async move { task.await.map_err(|_| ()) })
        }
        #[cfg(test)]
        {
            if self.fail.get() {
                return Task::ready(Err(()));
            }
            if let Some(rx) = self.delayed_read.borrow_mut().take() {
                return cx.background_executor().spawn(async move { rx.recv().await.map_err(|_| ()) });
            }
            Task::ready(Ok(self.entries.borrow().get(key).cloned()))
        }
    }

    pub fn write(&self, key: &str, login: &str, password: &[u8], cx: &App) -> Task<Result<(), ()>> {
        #[cfg(not(test))]
        {
            let task = cx.write_credentials(key, login, password);
            cx.background_executor().spawn(async move { task.await.map_err(|_| ()) })
        }
        #[cfg(test)]
        {
            if self.fail.get() {
                return Task::ready(Err(()));
            }
            if let Some(rx) = self.delayed_write.borrow_mut().take() {
                let entries = self.entries.clone();
                let (key, login, password) = (key.to_string(), login.to_string(), password.to_vec());
                return cx.foreground_executor().spawn(async move {
                    rx.recv().await.map_err(|_| ())?;
                    entries.borrow_mut().insert(key, (login, password));
                    Ok(())
                });
            }
            self.entries.borrow_mut().insert(key.into(), (login.into(), password.into()));
            Task::ready(Ok(()))
        }
    }

    pub fn delete(&self, key: &str, cx: &App) -> Task<Result<(), ()>> {
        #[cfg(not(test))]
        {
            let task = cx.delete_credentials(key);
            cx.background_executor().spawn(async move { task.await.map_err(|_| ()) })
        }
        #[cfg(test)]
        {
            let _ = cx;
            if self.fail.get() {
                return Task::ready(Err(()));
            }
            self.entries.borrow_mut().remove(key);
            Task::ready(Ok(()))
        }
    }
}
