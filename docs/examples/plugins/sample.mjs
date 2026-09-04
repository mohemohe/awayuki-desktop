export default {
  version: 1,
  beforeCreatePost: (obj) => {
    if (obj.visibility === "public" && obj.text.includes("内緒")) {
      obj.visibility = "unlisted";
    }
    return obj;
  },
  registerComposeButtons: [
    {
      icon: "🥹​",
      onClick: (obj) => {
        obj.cw_title = "ぴえん";
        return obj;
      },
    }
  ],
};
