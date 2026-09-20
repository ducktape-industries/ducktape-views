//! Live opt-in for host window and input-method observations.
/// Invoke inside the active subscription branch, including cached recipes.
/// This does not request permission for any host task effect.
pub fn observe<Message>(
    subscription: crate::Subscription<Message>,
    interest: crate::wire::events::Interest,
) -> crate::Subscription<Message> {
    crate::slots::include_event_interest(interest);
    subscription
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{App, Driver, Task};

    #[derive(Clone)]
    enum Message {
        Window(crate::wire::events::Window),
        Size((f32, f32)),
    }

    struct LifecycleApp {
        windows: Vec<crate::wire::events::Window>,
        sizes: Vec<(f32, f32)>,
    }

    impl App for LifecycleApp {
        type Message = Message;

        fn boot() -> (Self, Task<Self::Message>) {
            (
                Self {
                    windows: Vec::new(),
                    sizes: Vec::new(),
                },
                Task::none(),
            )
        }

        fn view(&self) -> crate::wire::Node {
            let size = crate::slots::handler(Box::new(|size| Some(Message::Size(size))));
            crate::wire::Node::Sensor {
                key: "layer/viewport".into(),
                reset: None,
                on_show: Some(size),
                on_resize: Some(size),
                on_hide: None,
                anticipate: None,
                delay: None,
                child: Box::new(crate::wire::Node::empty()),
            }
        }

        fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
            match message {
                Message::Window(event) => self.windows.push(event),
                Message::Size(size) => self.sizes.push(size),
            }
            Task::none()
        }

        fn subscription(&self) -> crate::Subscription<Self::Message> {
            observe(
                crate::Subscription::filter_events(|event| match event {
                    crate::wire::Event::Observation {
                        event: crate::wire::events::Event::Window(window),
                        ..
                    } => Some(Message::Window(window.clone())),
                    _ => None,
                }),
                crate::wire::events::Interest {
                    focus: true,
                    close: true,
                    ..Default::default()
                },
            )
        }
    }

    fn observation(event: crate::wire::events::Window) -> crate::wire::Event {
        crate::wire::Event::Observation {
            event: crate::wire::events::Event::Window(event),
            captured: false,
        }
    }

    #[test]
    fn layer_resize_and_window_lifecycle_observations_reach_the_active_instance() {
        let mut driver = Driver::<LifecycleApp>::new();
        let first = driver.tick(vec![]);
        assert_eq!(
            first.event_interest,
            crate::wire::events::Interest {
                focus: true,
                close: true,
                ..Default::default()
            }
        );
        let size_handler = match first.root.as_ref() {
            Some(crate::wire::Node::Sensor {
                on_show: Some(handler),
                on_resize: Some(resize),
                ..
            }) => {
                assert_eq!(handler, resize);
                *handler
            }
            root => panic!("viewport sensor missing: {root:?}"),
        };

        driver.tick(vec![crate::wire::Event::Size {
            handler: size_handler,
            width: 720.0,
            height: 480.0,
        }]);
        assert_eq!(driver.app.sizes, vec![(720.0, 480.0)]);

        driver.tick(vec![
            observation(crate::wire::events::Window::Focused),
            observation(crate::wire::events::Window::Unfocused),
            observation(crate::wire::events::Window::CloseRequested),
            observation(crate::wire::events::Window::Closed),
        ]);
        assert_eq!(
            driver.app.windows,
            vec![
                crate::wire::events::Window::Focused,
                crate::wire::events::Window::Unfocused,
                crate::wire::events::Window::CloseRequested,
                crate::wire::events::Window::Closed,
            ]
        );
    }

    #[test]
    fn lifecycle_events_and_resize_do_not_cross_driver_instances() {
        let mut first = Driver::<LifecycleApp>::new();
        let mut second = Driver::<LifecycleApp>::new();
        let first_frame = first.tick(vec![]);
        let second_frame = second.tick(vec![]);
        let handler = |frame: &crate::wire::Frame| match frame.root.as_ref() {
            Some(crate::wire::Node::Sensor {
                on_show: Some(handler),
                ..
            }) => *handler,
            root => panic!("viewport sensor missing: {root:?}"),
        };

        first.tick(vec![crate::wire::Event::Size {
            handler: handler(&first_frame),
            width: 640.0,
            height: 360.0,
        }]);
        first.tick(vec![observation(crate::wire::events::Window::Focused)]);

        assert_eq!(first.app.sizes, vec![(640.0, 360.0)]);
        assert_eq!(
            first.app.windows,
            vec![crate::wire::events::Window::Focused]
        );
        assert!(second.app.sizes.is_empty());
        assert!(second.app.windows.is_empty());
        assert_eq!(handler(&second_frame), handler(&second.tick(vec![])));
    }
}
