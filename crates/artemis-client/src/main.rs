mod mock;
mod theme;
mod views;

use iced::widget::{container, row};
use iced::{Element, Length, Task as IcedTask};

use mock::sample_data;
use views::{
    channel_sidebar, chat_area, member_list, server_list, ChannelSidebarMsg, ChatAreaMsg,
    MemberListMsg, ServerListMsg,
};

fn main() -> iced::Result {
    tracing_subscriber::fmt::init();

    iced::application("Artemis", Artemis::update, Artemis::view)
        .theme(|_| theme::artemis_theme())
        .window_size((1280.0, 720.0))
        .run_with(Artemis::new)
}

struct Artemis {
    servers: Vec<artemis_core::Server>,
    active_server_idx: usize,
    active_channel_id: Option<uuid::Uuid>,
    messages: Vec<artemis_core::Message>,
    members: Vec<artemis_core::ServerMember>,
    message_input: String,
    show_member_list: bool,
}

#[derive(Debug, Clone)]
enum Message {
    ServerList(ServerListMsg),
    ChannelSidebar(ChannelSidebarMsg),
    ChatArea(ChatAreaMsg),
    MemberList(MemberListMsg),
}

impl Artemis {
    fn new() -> (Self, IcedTask<Message>) {
        let (servers, messages, members) = sample_data();
        let active_channel_id = servers
            .first()
            .and_then(|s| s.categories.first())
            .and_then(|c| c.channels.first())
            .map(|ch| ch.id);

        (
            Self {
                servers,
                active_server_idx: 0,
                active_channel_id,
                messages,
                members,
                message_input: String::new(),
                show_member_list: true,
            },
            IcedTask::none(),
        )
    }

    fn update(&mut self, message: Message) -> IcedTask<Message> {
        match message {
            Message::ServerList(ServerListMsg::SelectServer(idx)) => {
                self.active_server_idx = idx;
            }
            Message::ChannelSidebar(ChannelSidebarMsg::SelectChannel(id)) => {
                self.active_channel_id = Some(id);
            }
            Message::ChannelSidebar(ChannelSidebarMsg::ToggleCategory(cat_id)) => {
                if let Some(server) = self.servers.get_mut(self.active_server_idx) {
                    if let Some(cat) = server.categories.iter_mut().find(|c| c.id == cat_id) {
                        cat.collapsed = !cat.collapsed;
                    }
                }
            }
            Message::ChatArea(ChatAreaMsg::InputChanged(val)) => {
                self.message_input = val;
            }
            Message::ChatArea(ChatAreaMsg::SendMessage) => {
                if !self.message_input.trim().is_empty() {
                    if let Some(channel_id) = self.active_channel_id {
                        let msg = artemis_core::models::message::Message {
                            id: uuid::Uuid::new_v4(),
                            channel_id,
                            author_id: uuid::Uuid::nil(),
                            author_name: "You".to_string(),
                            author_avatar: None,
                            content: self.message_input.clone(),
                            attachments: vec![],
                            timestamp: chrono::Utc::now(),
                            edited_at: None,
                        };
                        self.messages.push(msg);
                        self.message_input.clear();
                    }
                }
            }
            Message::MemberList(MemberListMsg::ToggleMemberList) => {
                self.show_member_list = !self.show_member_list;
            }
        }
        IcedTask::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let active_server = self.servers.get(self.active_server_idx);

        let server_list_view = server_list::view(&self.servers, self.active_server_idx)
            .map(Message::ServerList);

        let channel_sidebar_view = channel_sidebar::view(
            active_server,
            self.active_channel_id,
        )
        .map(Message::ChannelSidebar);

        let active_channel = active_server.and_then(|s| {
            s.categories
                .iter()
                .flat_map(|c| &c.channels)
                .find(|ch| Some(ch.id) == self.active_channel_id)
        });

        let chat_view = chat_area::view(
            active_channel,
            &self.messages,
            &self.message_input,
            self.active_channel_id,
        )
        .map(Message::ChatArea);

        let mut main_row = row![
            server_list_view,
            channel_sidebar_view,
            chat_view,
        ];

        if self.show_member_list {
            let member_view =
                member_list::view(&self.members).map(Message::MemberList);
            main_row = main_row.push(member_view);
        }

        container(main_row)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}
